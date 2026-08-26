//! OutputSink: `paste` | `copy_only` (Spec §5.1). `review` ist v2 und wird nicht vorbereitet.
#![allow(dead_code)]

use std::time::Instant;

use thiserror::Error;

/// Native Vordergrund-Kennung (HWND bzw. X11-Window), als portable Zahl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureContext {
    pub start_window_id: Option<WindowId>,
    pub target_window_id: Option<WindowId>,
    pub ended_at: Instant,
}

#[derive(Debug, Error)]
pub enum InjectError {
    #[error("Ausgabe fehlgeschlagen: {0}")]
    Failed(String),
}

pub trait OutputSink {
    fn paste(&mut self, text: &str, ctx: &CaptureContext) -> Result<(), InjectError>;
    fn copy_only(&mut self, text: &str) -> Result<(), InjectError>;
}

#[derive(Debug, Default)]
pub struct StubOutputSink;

impl OutputSink for StubOutputSink {
    fn paste(&mut self, _text: &str, _ctx: &CaptureContext) -> Result<(), InjectError> {
        Ok(())
    }

    fn copy_only(&mut self, _text: &str) -> Result<(), InjectError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_paste_and_copy_only_succeed() {
        let mut sink = StubOutputSink;
        let ctx = CaptureContext {
            start_window_id: Some(WindowId(1)),
            target_window_id: Some(WindowId(1)),
            ended_at: Instant::now(),
        };
        sink.paste("Hallo", &ctx).unwrap();
        sink.copy_only("Hallo").unwrap();
    }
}
