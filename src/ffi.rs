use std::ffi::{CStr, CString};
use std::os::raw::{c_char};
use std::{ptr};
use std::fs::OpenOptions;
use std::io::Write;

use serde::Serialize;

use crate::ms_data;
use crate::io::readers::{self, FrameWindowSplittingStrategy, SpectrumReaderConfig};
use crate::ms_data::AcquisitionType;

// Helper to log errors to a file
fn log_error(msg: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("C:\\temp\\timsrust_debug.log")
    {
        let _ = writeln!(file, "[{}] {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
    }
}

#[derive(Serialize)]
struct PrecursorOut {
    mz: f32,
    charge: Option<u8>,
    intensity: Option<f32>,
    spectrum_ref: Option<String>,
    inverse_ion_mobility: Option<f32>,
    isolation_window: Option<[f32; 2]>,
}

#[derive(Serialize)]
struct RawSpectrumOut {
    precursors: Vec<PrecursorOut>,
    scan_start_time: Option<f32>,
    ion_injection_time: Option<f32>,
    total_ion_current: f32,
    mz: Vec<f32>,
    ms_level: u8,
    id: String,
    intensity: Vec<f32>,
}

fn parse_precursor(dda_precursor: ms_data::Precursor, isolation_width: Option<f32>) -> PrecursorOut {
    PrecursorOut {
        mz: dda_precursor.mz as f32,
        charge: dda_precursor.charge.map(|x| x as u8),
        intensity: dda_precursor.intensity.map(|x| x as f32),
        spectrum_ref: Some(dda_precursor.frame_index.to_string()),
        inverse_ion_mobility: Some(dda_precursor.im as f32),
        isolation_window: isolation_width.map(|w| [-w / 2.0, w / 2.0]),
    }
}

// Read MS/MS spectra (DDA - no config needed)
#[no_mangle]
pub extern "C" fn read_msn_spectra(path: *const c_char) -> *mut c_char {
    log_error("=== read_msn_spectra called (DDA) ===");
    
    let result = std::panic::catch_unwind(|| {
        if path.is_null() {
            log_error("ERROR: path is null");
            return ptr::null_mut();
        }
        let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(s) => {
                log_error(&format!("Path: {}", s));
                s
            },
            Err(e) => {
                log_error(&format!("ERROR: Invalid UTF-8 in path: {:?}", e));
                return ptr::null_mut();
            }
        };

        log_error("Building DDA reader...");
        let reader = match readers::SpectrumReader::build()
            .with_path(path_str)
            .finalize()
        {
            Ok(r) => {
                log_error(&format!("Reader built successfully, {} spectra", r.len()));
                r
            },
            Err(e) => {
                log_error(&format!("ERROR: Failed to build reader: {:?}", e));
                return ptr::null_mut();
            }
        };

        process_spectra(&reader)
    });

    result.unwrap_or_else(|e| {
        log_error(&format!("PANIC caught: {:?}", e));
        ptr::null_mut()
    })
}

// Read MS/MS with DIA config (requires frame splitting strategy)
#[no_mangle]
pub extern "C" fn read_msn_spectra_with_config(path: *const c_char, config_json: *const c_char) -> *mut c_char {
    log_error("=== read_msn_spectra_with_config called (DIA) ===");
    
    let result = std::panic::catch_unwind(|| {
        if path.is_null() {
            log_error("ERROR: path is null");
            return ptr::null_mut();
        }
        let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(s) => {
                log_error(&format!("Path: {}", s));
                s
            },
            Err(e) => {
                log_error(&format!("ERROR: Invalid UTF-8 in path: {:?}", e));
                return ptr::null_mut();
            }
        };

        // Parse the config from JSON
        let config = if !config_json.is_null() {
            match unsafe { CStr::from_ptr(config_json) }.to_str() {
                Ok(s) if !s.trim().is_empty() => {
                    log_error(&format!("Config JSON: {}", s));
                    match serde_json::from_str::<SpectrumReaderConfig>(s) {
                        Ok(cfg) => {
                            log_error("Config parsed successfully");
                            cfg
                        },
                        Err(e) => {
                            log_error(&format!("ERROR: Failed to parse config JSON: {:?}", e));
                            return ptr::null_mut();
                        },
                    }
                },
                _ => {
                    log_error("No config provided, using default");
                    SpectrumReaderConfig::default()
                }
            }
        } else {
            log_error("config_json is null, using default");
            SpectrumReaderConfig::default()
        };

        log_error(&format!("Building DIA reader with config: {:?}", config));
        let reader = match readers::SpectrumReader::build()
            .with_path(path_str)
            .with_config(config)
            .finalize()
        {
            Ok(r) => {
                log_error(&format!("Reader built successfully, {} spectra", r.len()));
                r
            },
            Err(e) => {
                log_error(&format!("ERROR: Failed to build reader: {:?}", e));
                return ptr::null_mut();
            }
        };

        process_spectra(&reader)
    });

    result.unwrap_or_else(|e| {
        log_error(&format!("PANIC caught: {:?}", e));
        ptr::null_mut()
    })
}

// Shared processing logic
fn process_spectra(reader: &readers::SpectrumReader) -> *mut c_char {
    let mut out: Vec<RawSpectrumOut> = Vec::new();

    log_error("Reading spectra...");
    let limit = reader.len().min(10);
    for index in 0..limit {
        log_error(&format!("Reading spectrum {}/{}", index + 1, limit));
        match reader.get(index) {
            Ok(dda_spectrum) => {
                log_error(&format!("Got spectrum {}, has precursor: {}", index, dda_spectrum.precursor.is_some()));
                if let Some(dda_precursor) = dda_spectrum.precursor {
                    let isolation_width = Some(dda_spectrum.isolation_width as f32);
                    let scan_start_time = Some(dda_precursor.rt as f32 / 60.0);

                    let precursor = parse_precursor(dda_precursor, isolation_width);

                    let spectrum = RawSpectrumOut {
                        precursors: vec![precursor],
                        scan_start_time,
                        ion_injection_time: None,
                        total_ion_current: 0.0,
                        mz: dda_spectrum.mz_values.iter().map(|&x| x as f32).collect(),
                        ms_level: 2,
                        id: dda_spectrum.index.to_string(),
                        intensity: dda_spectrum.intensities.iter().map(|&x| x as f32).collect(),
                    };
                    out.push(spectrum);
                    log_error(&format!("Added spectrum {} with {} peaks", index, dda_spectrum.mz_values.len()));
                }
            }
            Err(e) => {
                log_error(&format!("ERROR: reading spectrum {}: {:?}", index, e));
            }
        }
    }

    log_error(&format!("Read {} spectra total, serializing...", out.len()));
    
    if out.is_empty() {
        log_error("WARNING: No spectra with precursors found");
    }
    
    match serde_json::to_string(&out) {
        Ok(s) => {
            log_error(&format!("Serialized {} bytes", s.len()));
            match CString::new(s) {
                Ok(cs) => {
                    log_error("Returning CString");
                    cs.into_raw()
                },
                Err(e) => {
                    log_error(&format!("ERROR: Failed to create CString: {:?}", e));
                    ptr::null_mut()
                }
            }
        },
        Err(e) => {
            log_error(&format!("ERROR: Failed to serialize JSON: {:?}", e));
            ptr::null_mut()
        }
    }
}

// Heuristic/metadata-based DIA detection
#[no_mangle]
pub extern "C" fn is_dia(path: *const c_char) -> bool {
    let result = std::panic::catch_unwind(|| {
        if path.is_null() {
            return false;
        }
        let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(s) => s,
            Err(_) => return false,
        };

        // Try to build a reader and check for DIA
        match readers::SpectrumReader::build()
            .with_path(path_str)
            .finalize()
        {
            Ok(reader) => {
                // Check a few spectra for DIA characteristics
                let limit = reader.len().min(10);
                for index in 0..limit {
                    if let Ok(s) = reader.get(index) {
                        if s.isolation_width > 0.0 {
                            return true;
                        }
                    }
                }
                false
            },
            Err(_) => false,
        }
    });

    result.unwrap_or(false)
}

// Free a CString produced by the FFI
#[no_mangle]
pub extern "C" fn tr_free_cstring(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_is_dia() {
        // Replace with an actual path to a .d folder on your machine
        let path = CString::new(r"K:\R00012_GlyCounter\Talus_NB_KELLY\240202_NB_KELLY_cyto_01.d\240202_NB_KELLY_cyto_01_S3-C1_1_9470.d").unwrap();
        let result = is_dia(path.as_ptr());
        println!("is_dia result: {}", result);
    }

    #[test]
    fn test_read_msn_spectra() {
        let path = CString::new(r"K:\R00012_GlyCounter\Talus_NB_KELLY\240202_NB_KELLY_cyto_01.d\240202_NB_KELLY_cyto_01_S3-C1_1_9470.d").unwrap();
        let result_ptr = read_msn_spectra(path.as_ptr());
        
        if result_ptr.is_null() {
            println!("read_msn_spectra returned null");
        } else {
            let result = unsafe { CStr::from_ptr(result_ptr) };
            println!("Result length: {}", result.to_bytes().len());
            println!("First 100 chars: {:?}", &result.to_str().unwrap()[..100.min(result.to_bytes().len())]);
            tr_free_cstring(result_ptr);
        }
    }

    #[test]
    fn test_read_msn_spectra_with_config() {
        let path = CString::new(r"K:\R00012_GlyCounter\Talus_NB_KELLY\240202_NB_KELLY_cyto_01.d\240202_NB_KELLY_cyto_01_S3-C1_1_9470.d").unwrap();
        let config = CString::new(r#"{"frame_splitting_params":{"Quadrupole":{"UniformMobility":[[0.1,0.05],null]}}}"#).unwrap();
        
        let result_ptr = read_msn_spectra_with_config(path.as_ptr(), config.as_ptr());
        
        if result_ptr.is_null() {
            println!("read_msn_spectra_with_config returned null");
        } else {
            let result = unsafe { CStr::from_ptr(result_ptr) };
            println!("Result length: {}", result.to_bytes().len());
            println!("First 100 chars: {:?}", &result.to_str().unwrap()[..100.min(result.to_bytes().len())]);
            tr_free_cstring(result_ptr);
        }
    }
}