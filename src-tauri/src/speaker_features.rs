//! Compact 16 kHz log-mel features for an experimental speaker encoder.
use rustfft::{FftPlanner, num_complex::Complex};

pub fn speaker_features(audio: &[f32]) -> Result<Vec<Vec<f32>>, &'static str> {
    if audio.len() < 400 || audio.iter().any(|x| !x.is_finite()) { return Err("invalid speaker audio"); }
    let hz_to_mel = |hz: f32| 1127.0 * (1.0 + hz / 700.0).ln();
    let lo = hz_to_mel(20.0);
    let hi = hz_to_mel(8000.0);
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(512);
    let mut rows = Vec::new();
    for frame in audio.windows(400).step_by(160) {
        let mean = frame.iter().sum::<f32>() / 400.0;
        let mut bins = vec![Complex::new(0.0, 0.0); 512];
        let mut prev = frame[0] - mean;
        for (i, &sample) in frame.iter().enumerate() {
            let value = sample - mean;
            let emphasized = value - 0.97 * prev;
            prev = value;
            let window = 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / 399.0).cos();
            bins[i].re = emphasized * window;
        }
        fft.process(&mut bins);
        let mut row = vec![0.0; 80];
        for (m, value) in row.iter_mut().enumerate() {
            let left = lo + (hi - lo) * m as f32 / 81.0;
            let center = lo + (hi - lo) * (m + 1) as f32 / 81.0;
            let right = lo + (hi - lo) * (m + 2) as f32 / 81.0;
            let mut energy = 0.0;
            for (k, bin) in bins.iter().enumerate().take(257) {
                let mel = hz_to_mel(k as f32 * 16000.0 / 512.0);
                let weight = ((mel - left) / (center - left)).min((right - mel) / (right - center)).max(0.0);
                energy += bin.norm_sqr() * weight;
            }
            *value = energy.max(f32::EPSILON).ln();
        }
        rows.push(row);
    }
    for channel in 0..80 {
        let mean = rows.iter().map(|row| row[channel]).sum::<f32>() / rows.len() as f32;
        for row in &mut rows { row[channel] -= mean; }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn features_are_finite_and_centered() {
        let audio: Vec<_> = (0..16000).map(|i| (i as f32 * 0.09).sin()).collect();
        let rows = speaker_features(&audio).unwrap();
        assert_eq!(rows.len(), 98);
        assert!(rows.iter().flatten().all(|value| value.is_finite()));
        assert!((rows.iter().map(|row| row[0]).sum::<f32>() / rows.len() as f32).abs() < 0.001);
    }
}
