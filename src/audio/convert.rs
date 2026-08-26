//! Kanal-Mittelung und Integer→f32 [-1,1] auf dem Worker (Spec §6.4).

pub fn i8_to_f32(sample: i8) -> f32 {
    f32::from(sample) / 128.0
}

pub fn u8_to_f32(sample: u8) -> f32 {
    (f32::from(sample) - 128.0) / 128.0
}

pub fn i16_to_f32(sample: i16) -> f32 {
    f32::from(sample) / 32768.0
}

pub fn u16_to_f32(sample: u16) -> f32 {
    (f32::from(sample) - 32768.0) / 32768.0
}

pub fn i32_to_f32(sample: i32) -> f32 {
    (f64::from(sample) / 2_147_483_648.0) as f32
}

pub fn u32_to_f32(sample: u32) -> f32 {
    ((f64::from(sample) - 2_147_483_648.0) / 2_147_483_648.0) as f32
}

pub fn i64_to_f32(sample: i64) -> f32 {
    (sample as f64 / 9_223_372_036_854_775_808.0) as f32
}

pub fn f64_to_f32(sample: f64) -> f32 {
    sample as f32
}

/// Interleaved → mono. 1 Kanal: Kopie. Sonst arithmetisches Mittel je Frame.
pub fn downmix_interleaved(interleaved: &[f32], channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    if channels == 1 {
        return interleaved.to_vec();
    }
    let frames = interleaved.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for frame in interleaved.chunks_exact(channels) {
        let sum: f32 = frame.iter().copied().sum();
        mono.push(sum / channels as f32);
    }
    mono
}

pub fn i16_interleaved_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().copied().map(i16_to_f32).collect()
}

pub fn u16_interleaved_to_f32(samples: &[u16]) -> Vec<f32> {
    samples.iter().copied().map(u16_to_f32).collect()
}

pub fn i32_interleaved_to_f32(samples: &[i32]) -> Vec<f32> {
    samples.iter().copied().map(i32_to_f32).collect()
}

pub fn i8_interleaved_to_f32(samples: &[i8]) -> Vec<f32> {
    samples.iter().copied().map(i8_to_f32).collect()
}

pub fn u8_interleaved_to_f32(samples: &[u8]) -> Vec<f32> {
    samples.iter().copied().map(u8_to_f32).collect()
}

pub fn u32_interleaved_to_f32(samples: &[u32]) -> Vec<f32> {
    samples.iter().copied().map(u32_to_f32).collect()
}

pub fn i64_interleaved_to_f32(samples: &[i64]) -> Vec<f32> {
    samples.iter().copied().map(i64_to_f32).collect()
}

pub fn f64_interleaved_to_f32(samples: &[f64]) -> Vec<f32> {
    samples.iter().copied().map(f64_to_f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i16_full_scale_maps_to_unit_range() {
        assert!((i16_to_f32(0)).abs() < f32::EPSILON);
        assert!((i16_to_f32(i16::MAX) - 32767.0 / 32768.0).abs() < 1e-6);
        assert!((-1.0 - i16_to_f32(i16::MIN)).abs() < 1e-6);
        assert!((-1.0..=1.0).contains(&i16_to_f32(i16::MIN)));
        assert!((-1.0..=1.0).contains(&i16_to_f32(i16::MAX)));
    }

    #[test]
    fn stereo_equal_channels_match_mono() {
        let mono = [0.2_f32, -0.4, 0.8];
        let mut stereo = Vec::new();
        for s in mono {
            stereo.push(s);
            stereo.push(s);
        }
        let down = downmix_interleaved(&stereo, 2);
        for (a, b) in down.iter().zip(mono) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn stereo_antiphase_cancels() {
        let mut stereo = Vec::new();
        for s in [0.5_f32, -0.25, 0.9] {
            stereo.push(s);
            stereo.push(-s);
        }
        let down = downmix_interleaved(&stereo, 2);
        assert!(down.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn i8_u8_full_scale() {
        assert!((-1.0 - i8_to_f32(i8::MIN)).abs() < 1e-6);
        assert!((i8_to_f32(i8::MAX) - 127.0 / 128.0).abs() < 1e-6);
        assert!((u8_to_f32(128)).abs() < 1e-6);
        assert!((-1.0 - u8_to_f32(0)).abs() < 1e-6);
        assert!((u8_to_f32(255) - 127.0 / 128.0).abs() < 1e-6);
    }

    #[test]
    fn i32_u16_u32_i64_f64_range() {
        assert!((-1.0 - i32_to_f32(i32::MIN)).abs() < 1e-6);
        assert!(i32_to_f32(0).abs() < 1e-6);
        assert!(i32_to_f32(i32::MAX) > 0.99);
        assert!((u16_to_f32(0) + 1.0).abs() < 1e-6);
        assert!(u16_to_f32(32768).abs() < 1e-6);
        assert!(u16_to_f32(u16::MAX) > 0.99);
        assert!((u32_to_f32(0) + 1.0).abs() < 1e-5);
        assert!(u32_to_f32(1 << 31).abs() < 1e-5);
        assert!((-1.0 - i64_to_f32(i64::MIN)).abs() < 1e-5);
        assert!(i64_to_f32(0).abs() < 1e-6);
        assert!((f64_to_f32(-0.5) + 0.5).abs() < 1e-6);
        assert!((-1.0..=1.0).contains(&i32_to_f32(i32::MIN)));
        assert!((-1.0..=1.0).contains(&u16_to_f32(0)));
    }
}
