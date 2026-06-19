## Lumis Camera Android App - Calibration System State Summary

### Project Overview
Implementing a robust colour calibration system for the Lumis camera Android app that handles:
- Colour target scanning and calibration
- Automatic download of encrypted calibration files
- File re-encryption with device-specific keys
- Automatic deletion of corrupt files
- Proper error handling and recovery

### Custom Instructions from User
3. **Auto-Delete Corrupt Files** - Automatically remove invalid calibration files
4. **Keep Logic in Rust** - Minimize Kotlin code, maximize Rust. Download must happen thru Kotlin due to platform restrictions
5. **Clean Code Blocks** - Provide code in clean blocks without comments or clutter. Use format:
   Add/remove/change to `/path/to/file` `function_name`:
   ```
{changes to function
}
```
### Current Architecture

#### Data Flow
1. **Rust Side** (`lib.rs` and `chameleon.rs`):
   - Maintains calibration state
   - Performs target scanning and calibration
   - Handles file encryption/decryption
   - Auto-deletes corrupt files
   - Returns status codes to Kotlin

2. **Kotlin Side** (`RustProcessor.kt` and `UserInterface.kt`):
   - Calls Rust functions via JNI
   - Handles network downloads (platform requirement)
   - Checks status codes and responds accordingly


### Calibration Flow

1. **User Initiates Calibration**
   - Kotlin calls `RustProcessor.calibrate()`

2. **Rust Attempts Calibration**
   - find and read colour target
   - Attempts to load calibration files required for illuminants, standard observers, target type/serial etc.
   - If calibration files are missing/corrupt: generates download info and relays to Kotlin for download

3. **File Reading (`read_cal`)**
   - Calls `make_cal_local()` to get local file path
   - If file exists: calls `read_cal_file()`
   - If file corrupt: **auto-deletes** and returns error
   - If file missing: returns download info

4. **Download Needed**
   - `calibrate()` sets status to `CAL_STATUS_NEEDS_DOWNLOAD`
   - Stores URL, path, and encryption keys in CalibrationState
   - Returns false to Kotlin (this is up for restructuring to get the downloads proper)

5. **Kotlin Checks Status**
   - Calls `getCalibrationStatus()`
   - If `CAL_STATUS_NEEDS_DOWNLOAD`:
     - Gets URL via `getDownloadUrl()`
     - Gets path via `getDownloadPath()`
     - Downloads file
     - Calls `processDownloadedFile()` (this is also up for restructuring)

6. **File Processing**
   - `processDownloadedFile()` reads downloaded file
   - Decrypts with remote key
   - Re-encrypts with local key
   - Saves to same path
   - Updates status to `CAL_STATUS_OK`

7. **Continue Calibration**
   - Kotlin downloads all files needed for Chameleon
   - This time files exist and are properly encrypted on disk
   - Calibration proceeds normally

### File Encryption Details

#### Remote File (on server)
- Encrypted with `remote_key` derived from target type/serial (for some files)
- Standard format for all devices

#### Local File (on device)
- Re-encrypted with `local_key` derived from:
  - Target type/serial
  - Device UUID
- Device-specific encryption

### Error Recovery Strategy

1. **Corrupt File**: Auto-delete → Trigger re-download
2. **Download Failed**: Set status → User can retry
3. **Decryption Failed**: Delete file → Trigger re-download
4. **Write Failed**: Delete partial file → Set error status

### Next Steps

1. Test full calibration flow with all profiles syncronizing to proper encrypted local
2. Verify file deletion and re-download works for corrupted files
3. Add more detailed logging for debugging
4. Consider adding retry logic for network failures
5. Add progress callbacks for download status (last steps once downloads are working)
