//! Minimal raw-image descriptor consumed by the demosaic code.
//!
//! The RCD demosaicer ([crate::debayer::rcd]) only needs the frame dimensions; the actual
//! pixel data lives in the [crate::debayer::rcd::RcdData] grid the caller fills in. Keep this
//! struct tiny and dependency-free so the demosaic module stays self-contained.
pub struct RawImage {
    pub width: usize,
    pub height: usize,
}

impl RawImage {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height }
    }
}
