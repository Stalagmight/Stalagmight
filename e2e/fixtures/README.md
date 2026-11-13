# E2E Test Fixtures

## DMP Files

Test DMP files for Stalagmight E2E testing. These are binary files from the MNemo cave surveying device.

### Available Test Files

| File | Version | Description |
|------|---------|-------------|
| `test_v2.dmp` | 2 | DMP version 2 format with multiple sections |
| `test_v5.dmp` | 5 | DMP version 5 format with LRUD data |
| `test_v5_buggy_EOS.dmp` | 5 | Version 5 with buggy end-of-section marker |
| `test_corrupted.dmp` | N/A | Invalid file for error handling tests |

### DMP Format

The DMP format is binary and contains:
- File header with version info
- Section metadata (name, surveyor, date)
- Shot data (azimuth, length, depth, LRUD, pitch, comments)

See `src-tauri/src/parsers/dmp.rs` for full format documentation.

## Stalagmight Projects

`fractal_plant_iter3.stalx` is the tracked, current-schema project used by the
agent harness's default `seed(mode: "fixture")` flow and stress scenarios.

### Usage in Tests

**Agent harness (True E2E)**:
The real Tauri backend parses these files. Journeys and scenarios use direct paths to import.

**Playwright (Mocked)**:
The mock IPC layer returns simulated section data. These files are available for reference but mocks don't actually read them.
