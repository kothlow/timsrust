use std::ffi::{CStr, CString};
use std::os::raw::{c_char};
use std::{ptr};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::collections::HashMap;

use serde::Serialize;

use crate::ms_data;
use crate::io::readers::{self};

// Helper to log errors to a file
fn log_error(msg: &str) {
    let log_path = std::env::var_os("TIMSRUST_DEBUG_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("timsrust_debug.log"));

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = writeln!(file, "[{}] {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
    }
}

// Global storage for readers - use handle-based access
lazy_static::lazy_static! {
    static ref READERS: Mutex<HashMap<usize, readers::SpectrumReader>> = Mutex::new(HashMap::new());
    static ref NEXT_HANDLE: Mutex<usize> = Mutex::new(1);
}

#[derive(Serialize)]
struct PrecursorOut {
    mz: f32,
    charge: Option<u8>,
    intensity: Option<f32>,
    spectrum_ref: Option<String>,
    ion_mobility: Option<f32>,
    isolation_window: Option<[f32; 2]>,
}

#[derive(Serialize)]
struct RawSpectrumOut {
    precursors: Vec<PrecursorOut>,
    scan_start_time: Option<f32>,
    mz: Vec<f32>,
    ms_level: u8,
    id: String,
    intensity: Vec<f32>,
    collision_energy: Option<f32>,
}

fn parse_precursor(dda_precursor: ms_data::Precursor, isolation_width: Option<f32>) -> PrecursorOut {
    PrecursorOut {
        mz: dda_precursor.mz as f32,
        charge: dda_precursor.charge.map(|x| x as u8),
        intensity: dda_precursor.intensity.map(|x| x as f32),
        spectrum_ref: Some(dda_precursor.frame_index.to_string()),
        ion_mobility: Some(dda_precursor.im as f32),
        isolation_window: isolation_width.map(|w| [-w / 2.0, w / 2.0]),
    }
}

// Open a reader and return a handle
#[no_mangle]
pub extern "C" fn open_reader(path: *const c_char) -> usize {
    let result = std::panic::catch_unwind(|| {
        if path.is_null() {
            log_error("ERROR: path is null");
            return 0;
        }
        let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(s) => {
                log_error(&format!("Opening reader for: {}", s));
                s
            },
            Err(e) => {
                log_error(&format!("ERROR: Invalid UTF-8 in path: {:?}", e));
                return 0;
            }
        };

        let reader = match readers::SpectrumReader::build()
            .with_path(path_str)
            .finalize()
        {
            Ok(r) => {
                log_error(&format!("Reader opened successfully, {} spectra", r.len()));
                r
            },
            Err(e) => {
                log_error(&format!("ERROR: Failed to build reader: {:?}", e));
                return 0;
            }
        };

        // Store reader and return handle
        let mut next_handle = NEXT_HANDLE.lock().unwrap();
        let handle = *next_handle;
        *next_handle += 1;
        
        READERS.lock().unwrap().insert(handle, reader);
        log_error(&format!("Assigned handle: {}", handle));
        handle
    });

    result.unwrap_or(0)
}

// Get the number of spectra in a reader
#[no_mangle]
pub extern "C" fn get_spectrum_count(handle: usize) -> usize {
    let readers = READERS.lock().unwrap();
    match readers.get(&handle) {
        Some(reader) => reader.len(),
        None => {
            log_error(&format!("ERROR: Invalid handle: {}", handle));
            0
        }
    }
}

// Get a single spectrum by index as JSON
#[no_mangle]
pub extern "C" fn get_spectrum(handle: usize, index: usize) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        let readers = READERS.lock().unwrap();
        let reader = match readers.get(&handle) {
            Some(r) => r,
            None => {
                log_error(&format!("ERROR: Invalid handle: {}", handle));
                return ptr::null_mut();
            }
        };

        if index >= reader.len() {
            log_error(&format!("ERROR: Index {} out of bounds (max: {})", index, reader.len()));
            return ptr::null_mut();
        }

        match reader.get(index) {
            Ok(spectrum) => {
                if let Some(dda_precursor) = spectrum.precursor {
                    let isolation_width = Some(spectrum.isolation_width as f32);
                    let scan_start_time = Some(dda_precursor.rt as f32 / 60.0);
                    let precursor = parse_precursor(dda_precursor, isolation_width);

                    let raw_spectrum = RawSpectrumOut {
                        precursors: vec![precursor],
                        scan_start_time,
                        mz: spectrum.mz_values.iter().map(|&x| x as f32).collect(),
                        ms_level: 2,
                        id: spectrum.index.to_string(),
                        intensity: spectrum.intensities.iter().map(|&x| x as f32).collect(),
                        collision_energy: Some(spectrum.collision_energy as f32),
                    };

                    match serde_json::to_string(&raw_spectrum) {
                        Ok(s) => match CString::new(s) {
                            Ok(cs) => cs.into_raw(),
                            Err(e) => {
                                log_error(&format!("ERROR: Failed to create CString: {:?}", e));
                                ptr::null_mut()
                            }
                        },
                        Err(e) => {
                            log_error(&format!("ERROR: Failed to serialize spectrum: {:?}", e));
                            ptr::null_mut()
                        }
                    }
                } else {
                    // Return null for spectra without precursors
                    ptr::null_mut()
                }
            }
            Err(e) => {
                log_error(&format!("ERROR: Failed to read spectrum {}: {:?}", index, e));
                ptr::null_mut()
            }
        }
    });

    result.unwrap_or(ptr::null_mut())
}

// Close a reader and free resources
#[no_mangle]
pub extern "C" fn close_reader(handle: usize) {
    let mut readers = READERS.lock().unwrap();
    if readers.remove(&handle).is_some() {
        log_error(&format!("Closed reader handle: {}", handle));
    } else {
        log_error(&format!("ERROR: Tried to close invalid handle: {}", handle));
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

        log_error(&format!("Checking if DIA: {}", path_str));

        match readers::QuadrupoleSettingsReader::new(path_str) {
            Ok(quad_reader) => {
                let has_settings = quad_reader.len() > 0;
                log_error(&format!("QuadrupoleSettingsReader found {} settings", quad_reader.len()));
                if has_settings {
                    log_error("Has quadrupole settings - likely DIA");
                    return true;
                }
            },
            Err(e) => {
                log_error(&format!("No QuadrupoleSettingsReader (expected for DDA): {:?}", e));
            }
        }
        
        match readers::SpectrumReader::build()
            .with_path(path_str)
            .finalize()
        {
            Ok(reader) => {
                log_error(&format!("Built reader with {} spectra", reader.len()));
                let mut rt_to_count: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
                let limit = reader.len().min(100);
                
                for index in 0..limit {
                    if let Ok(s) = reader.get(index) {
                        if let Some(precursor) = &s.precursor {
                            let rt_key = (precursor.rt * 100.0) as i64;
                            *rt_to_count.entry(rt_key).or_insert(0) += 1;
                        }
                    }
                }
                
                log_error(&format!("Found {} unique RT bins", rt_to_count.len()));
                
                for (rt, count) in rt_to_count.iter() {
                    if *count > 3 {
                        log_error(&format!("Found {} spectra at RT {} - likely DIA", count, *rt as f64 / 100.0));
                        return true;
                    }
                }
                
                log_error("No DIA pattern found - likely DDA");
                false
            },
            Err(e) => {
                log_error(&format!("Error building reader for heuristic: {:?}", e));
                false
            }
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

// Keep old functions for backwards compatibility but mark as deprecated
#[no_mangle]
#[deprecated]
pub extern "C" fn read_msn_spectra(_path: *const c_char) -> *mut c_char {
    log_error("WARNING: read_msn_spectra is deprecated, use open_reader/get_spectrum/close_reader instead");
    ptr::null_mut()
}

#[no_mangle]
#[deprecated]
pub extern "C" fn read_msn_spectra_with_config(_path: *const c_char, _config_json: *const c_char) -> *mut c_char {
    log_error("WARNING: read_msn_spectra_with_config is deprecated, use open_reader/get_spectrum/close_reader instead");
    ptr::null_mut()
}