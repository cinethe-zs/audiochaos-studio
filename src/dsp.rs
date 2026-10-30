//! Signal analysis: K-weighted loudness, a Fourier transform, spectra and
//! percentiles.

use std::f64::consts::PI;

/// A second-order filter, run from rest.
pub struct Biquad {
    pub b: [f64; 3],
    /// With `a[0]` = 1.
    pub a: [f64; 3],
}

impl Biquad {
    pub fn run(&self, input: &[f64]) -> Vec<f64> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        input
            .iter()
            .map(|&x0| {
                let y0 = self.b[0] * x0 + self.b[1] * x1 + self.b[2] * x2
                    - self.a[1] * y1
                    - self.a[2] * y2;
                (x2, x1, y2, y1) = (x1, x0, y1, y0);
                y0
            })
            .collect()
    }
}

/// ITU-R BS.1770 K-weighting at 48 kHz: a high shelf, then a high-pass.
pub const SHELF: Biquad = Biquad {
    b: [1.53512485958697, -2.69169618940638, 1.19839281085285],
    a: [1.0, -1.69065929318241, 0.73248077421585],
};
pub const HIGH_PASS: Biquad = Biquad {
    b: [1.0, -2.0, 1.0],
    a: [1.0, -1.99004745483398, 0.99007225036621],
};

/// Samples in 100 ms at 48 kHz.
const HOP: usize = 4800;

/// Momentary loudness (LUFS of 400 ms windows) every 100 ms, from the
/// channels of a 48 kHz signal.
pub fn momentary(channels: &[Vec<f64>]) -> Vec<f64> {
    let hops = channels.first().map_or(0, |c| c.len() / HOP);
    let mut power = vec![0.0; hops];
    for channel in channels {
        let weighted = HIGH_PASS.run(&SHELF.run(channel));
        for (total, hop) in power.iter_mut().zip(weighted.as_chunks::<HOP>().0) {
            *total += hop.iter().map(|s| s * s).sum::<f64>() / HOP as f64;
        }
    }
    power
        .windows(4)
        .map(|window| -0.691 + 10.0 * (window.iter().sum::<f64>() / 4.0).max(1e-12).log10())
        .collect()
}

/// The `q`th percentile, interpolating between the closest values.
pub fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let position = q / 100.0 * (sorted.len() - 1) as f64;
    let (low, high) = (position.floor() as usize, position.ceil() as usize);
    sorted[low] + (sorted[high] - sorted[low]) * (position - low as f64)
}

/// In-place Fourier transform; the length must be a power of two.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        for k in 0..half {
            let angle = -2.0 * PI * k as f64 / len as f64;
            let (wr, wi) = (angle.cos(), angle.sin());
            for start in (0..n).step_by(len) {
                let (a, b) = (start + k, start + k + half);
                let (tr, ti) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// Power spectrum by Welch's method: Hann windows of `n` samples, half
/// overlapping, each with its mean removed. One value per bin, n / 2 + 1 of
/// them, bin `k` at `k` × rate / `n`; scaled up to a constant.
pub fn welch(signal: &[f64], n: usize) -> Vec<f64> {
    let window: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos())
        .collect();
    let mut spectrum = vec![0.0; n / 2 + 1];
    let mut segments = 0;
    let mut start = 0;
    while start + n <= signal.len() {
        let segment = &signal[start..start + n];
        let mean = segment.iter().sum::<f64>() / n as f64;
        let mut re: Vec<f64> = segment
            .iter()
            .zip(&window)
            .map(|(s, w)| (s - mean) * w)
            .collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        for (k, power) in spectrum.iter_mut().enumerate() {
            let p = re[k] * re[k] + im[k] * im[k];
            *power += if k == 0 || k == n / 2 { p } else { 2.0 * p };
        }
        segments += 1;
        start += n / 2;
    }
    for power in &mut spectrum {
        *power /= f64::from(segments.max(1));
    }
    spectrum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_the_definition() {
        let signal: Vec<f64> = (0..16).map(|i| ((i * 7) % 5) as f64 - 2.0).collect();
        let (mut re, mut im) = (signal.clone(), vec![0.0; 16]);
        fft(&mut re, &mut im);
        for k in 0..16 {
            let (mut sr, mut si) = (0.0, 0.0);
            for (n, x) in signal.iter().enumerate() {
                let angle = -2.0 * PI * (k * n) as f64 / 16.0;
                sr += x * angle.cos();
                si += x * angle.sin();
            }
            assert!(
                (re[k] - sr).abs() < 1e-9 && (im[k] - si).abs() < 1e-9,
                "bin {k}"
            );
        }
    }

    #[test]
    fn a_sine_reads_as_the_standard_says() {
        // EBU: a 997 Hz sine peaking at 0.1 on one channel is -23 LUFS.
        let sine: Vec<f64> = (0..96_000)
            .map(|i| 0.1 * (2.0 * PI * 997.0 * i as f64 / 48_000.0).sin())
            .collect();
        let levels = momentary(&[sine]);
        assert!((levels[10] + 23.0).abs() < 0.1, "{}", levels[10]);
    }

    #[test]
    fn percentiles_interpolate() {
        assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 50.0), 2.5);
        assert!((percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 90.0) - 4.6).abs() < 1e-12);
    }

    #[test]
    fn welch_finds_a_sine() {
        let sine: Vec<f64> = (0..48_000)
            .map(|i| (2.0 * PI * 1500.0 * i as f64 / 48_000.0).sin())
            .collect();
        let spectrum = welch(&sine, 1024);
        let peak = (0..spectrum.len())
            .max_by(|&a, &b| spectrum[a].total_cmp(&spectrum[b]))
            .unwrap();
        assert_eq!(peak, 32, "1500 Hz is bin 32 at 1024 points");
    }
}
