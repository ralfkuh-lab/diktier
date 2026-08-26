//! rubato FFT 16 kHz, Flush beim Stop. Kein Linear-Fallback (Spec §6.4).

use rubato::{FftFixedIn, Resampler};

use super::{AudioError, ENGINE_RATE};

const CHUNK_IN: usize = 1024;
const SUB_CHUNKS: usize = 2;

pub struct To16k {
    in_rate: u32,
    resampler: Option<FftFixedIn<f32>>,
}

impl To16k {
    pub fn new(in_rate: u32) -> Result<Self, AudioError> {
        if in_rate == 0 {
            return Err(AudioError::Failed("Eingangsrate 0".into()));
        }
        if in_rate == ENGINE_RATE {
            return Ok(Self {
                in_rate,
                resampler: None,
            });
        }
        let resampler = FftFixedIn::<f32>::new(
            usize::try_from(in_rate).unwrap_or(1),
            usize::try_from(ENGINE_RATE).unwrap_or(16_000),
            CHUNK_IN,
            SUB_CHUNKS,
            1,
        )
        .map_err(|e| AudioError::Failed(format!("rubato: {e}")))?;
        Ok(Self {
            in_rate,
            resampler: Some(resampler),
        })
    }

    pub fn process_all(&mut self, mono: &[f32]) -> Result<Vec<f32>, AudioError> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(mono.to_vec());
        };
        let mut out = Vec::with_capacity(expected_len(mono.len(), self.in_rate));
        let mut pos = 0;
        while pos < mono.len() {
            let needed = resampler.input_frames_next();
            if needed == 0 {
                break;
            }
            let remaining = mono.len() - pos;
            if remaining >= needed {
                let chunk = &mono[pos..pos + needed];
                let waves = resampler
                    .process(&[chunk], None)
                    .map_err(|e| AudioError::Failed(format!("rubato process: {e}")))?;
                if let Some(ch) = waves.first() {
                    out.extend_from_slice(ch);
                }
                pos += needed;
            } else {
                break;
            }
        }
        let leftover = &mono[pos..];
        if !leftover.is_empty() {
            let waves = resampler
                .process_partial(Some(&[leftover]), None)
                .map_err(|e| AudioError::Failed(format!("rubato partial: {e}")))?;
            if let Some(ch) = waves.first() {
                out.extend_from_slice(ch);
            }
        }
        let flushed = resampler
            .process_partial::<&[f32]>(None, None)
            .map_err(|e| AudioError::Failed(format!("rubato flush: {e}")))?;
        if let Some(ch) = flushed.first() {
            out.extend_from_slice(ch);
        }
        // FFT-Overlap erzeugt Latenz vorne und Rest hinten. Nach Flush auf die
        // erwartete Länge schneiden (Spec-Gate ±1 %), Delay vorn verwerfen.
        let expected = expected_len(mono.len(), self.in_rate);
        let delay = resampler.output_delay();
        if out.len() > expected {
            let extra = out.len() - expected;
            let skip = extra.min(delay);
            out.drain(..skip);
            out.truncate(expected);
        }
        Ok(out)
    }
}

pub fn expected_len(input_frames: usize, in_rate: u32) -> usize {
    if in_rate == 0 {
        return 0;
    }
    (input_frames as u64 * u64::from(ENGINE_RATE) / u64::from(in_rate)) as usize
}

pub fn resample_mono_to_16k(mono: &[f32], in_rate: u32) -> Result<Vec<f32>, AudioError> {
    To16k::new(in_rate)?.process_all(mono)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_at_engine_rate() {
        let input = vec![0.1, -0.2, 0.3];
        let out = resample_mono_to_16k(&input, ENGINE_RATE).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn zero_rate_is_error() {
        match To16k::new(0) {
            Err(crate::audio::AudioError::Failed(msg)) => assert!(msg.contains("0")),
            Err(other) => panic!("{other:?}"),
            Ok(_) => panic!("expected error"),
        }
    }
}
