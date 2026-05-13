use std::ffi::{CStr, CString};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use timsrust::{close_reader, get_spectrum, get_spectrum_count, open_reader, tr_free_cstring};

struct ReaderHandle(usize);

impl Drop for ReaderHandle {
    fn drop(&mut self) {
        if self.0 != 0 {
            close_reader(self.0);
        }
    }
}

fn log_to_debug(msg: &str) {
    let log_path = std::env::var_os("TIMSRUST_DEBUG_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("timsrust_debug.log"));

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = writeln!(file, "[TEST] {}", msg);
    }
}

#[test]
#[ignore]
fn ffi_smoke_test_against_local_raw_file() {
    let raw_path = std::env::var("TIMSRUST_FFI_RAW_FILE")
        .expect("Set TIMSRUST_FFI_RAW_FILE to a local .d or .ms2 file/folder before running this test");
    let raw_path = CString::new(raw_path).expect("raw file path must not contain NUL bytes");

    let handle = open_reader(raw_path.as_ptr());
    assert_ne!(handle, 0, "open_reader returned an invalid handle");
    let handle = ReaderHandle(handle);

    let spectrum_count = get_spectrum_count(handle.0);
    assert!(spectrum_count > 0, "reader should expose at least one spectrum");

    let spectrum_ptr = get_spectrum(handle.0, 0);
    assert!(!spectrum_ptr.is_null(), "first spectrum should serialize to JSON");

    let spectrum_json = unsafe { CStr::from_ptr(spectrum_ptr) }
        .to_string_lossy()
        .into_owned();
    tr_free_cstring(spectrum_ptr);

    log_to_debug(&format!("Sample spectrum JSON: {}", spectrum_json));

    let spectrum: serde_json::Value = serde_json::from_str(&spectrum_json)
        .expect("spectrum should be valid JSON");

    assert_eq!(spectrum["ms_level"], 2);
    assert!(spectrum["mz"].as_array().map(|values| !values.is_empty()).unwrap_or(false));
    assert!(spectrum["intensity"].as_array().map(|values| !values.is_empty()).unwrap_or(false));
    
    let precursors = spectrum["precursors"].as_array().expect("precursors should be an array");
    assert!(!precursors.is_empty(), "precursors array should not be empty");
    
    // Check first precursor has ion mobility data
    let first_precursor = &precursors[0];
    let ion_mobility = first_precursor["ion_mobility"].as_f64();
    log_to_debug(&format!("First precursor ion_mobility: {:?}", ion_mobility));
    assert!(
        first_precursor["ion_mobility"].is_number(),
        "first precursor should have ion_mobility as a number"
    );
    
    // Check ion injection time (optional but should be present in most cases)
    let injection_time = spectrum["ion_injection_time"].as_f64();
    log_to_debug(&format!("Spectrum ion_injection_time: {:?}", injection_time));
    if !spectrum["ion_injection_time"].is_null() {
        assert!(
            spectrum["ion_injection_time"].is_number(),
            "ion_injection_time should be a number if present"
        );
    }
}