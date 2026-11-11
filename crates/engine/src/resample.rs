//! Clock-drift compensation between the input device's clock and the output's.
//!
//! The capture side pushes samples at the mic's real rate, the render side pulls
//! at the cable's real rate; they're both "48 kHz" but never exactly. A PI
//! controller watches how full the ring buffer is and nudges a fractional
//! resampling ratio (a few hundred ppm at most, inaudible) to hold it at a target.

use rtrb::Consumer;

/// Max correction either way. Real clocks differ by well under 100 ppm.
const MAX_PPM: f64 = 2000.0;
/// Fill smoothing time constant (s); capture arrives in bursts, so average them out.
const FILL_TAU_S: f64 = 0.5;
const KP: f64 = 0.5;
const KI: f64 = 0.05;
/// If the buffer grows this far past target (s), drop the excess outright.
const OVERFILL_S: f64 = 0.08;

pub struct DriftResampler {
    sr: f64,
    target: f64,
    /// Cubic window: x[-1], x[0], x[1], x[2]; output is between x[0] and x[1].
    win: [f32; 4],
    frac: f64,
    ratio: f64,
    fill_avg: f64,
    integ: f64,
    primed: bool,
    started: bool,
}

pub struct RenderStats {
    pub underrun: bool,
    pub dropped: usize,
}

impl DriftResampler {
    pub fn new(sr: u32) -> Self {
        DriftResampler {
            sr: sr as f64,
            target: 0.0,
            win: [0.0; 4],
            frac: 0.0,
            ratio: 1.0,
            fill_avg: 0.0,
            integ: 0.0,
            primed: false,
            started: false,
        }
    }

    /// Frames to keep buffered. Set from the device periods.
    pub fn set_target(&mut self, frames: usize) {
        self.target = frames as f64;
    }

    pub fn ratio_ppm(&self) -> f64 {
        (self.ratio - 1.0) * 1e6
    }

    pub fn fill_avg(&self) -> f64 {
        self.fill_avg
    }

    fn shift(&mut self, x: f32) {
        self.win = [self.win[1], self.win[2], self.win[3], x];
    }

    /// Fill `out` from `src`, adjusting the ratio from the current fill level.
    pub fn render(&mut self, src: &mut Consumer<f32>, out: &mut [f32]) -> RenderStats {
        let mut stats = RenderStats { underrun: false, dropped: 0 };
        let fill = src.slots() as f64;

        if !self.primed {
            // Wait until the buffer reaches target before playing.
            if fill < self.target {
                out.fill(0.0);
                return stats;
            }
            self.primed = true;
            self.fill_avg = fill;
            self.integ = 0.0;
            if !self.started {
                self.started = true;
                for _ in 0..4 {
                    let x = src.pop().unwrap_or(0.0);
                    self.shift(x);
                }
            }
        }

        // Way too much buffered (e.g. render stalled): drop down to target.
        if fill > self.target + OVERFILL_S * self.sr {
            let excess = (fill - self.target) as usize;
            if let Ok(chunk) = src.read_chunk(excess) {
                chunk.commit_all();
            }
            stats.dropped = excess;
            self.fill_avg = self.target;
            self.integ = 0.0;
        }

        // PI control on the smoothed fill level.
        let dt = out.len() as f64 / self.sr;
        let a = 1.0 - (-dt / FILL_TAU_S).exp();
        self.fill_avg += (src.slots() as f64 - self.fill_avg) * a;
        let err_s = (self.fill_avg - self.target) / self.sr;
        self.integ = (self.integ + err_s * dt).clamp(-0.02, 0.02);
        let adj = (KP * err_s + KI * self.integ).clamp(-MAX_PPM * 1e-6, MAX_PPM * 1e-6);
        self.ratio = 1.0 + adj;

        for (i, y) in out.iter_mut().enumerate() {
            *y = hermite(&self.win, self.frac as f32);
            self.frac += self.ratio;
            while self.frac >= 1.0 {
                self.frac -= 1.0;
                match src.pop() {
                    Ok(x) => self.shift(x),
                    Err(_) => {
                        // Out of input: fade nothing fancy, just silence the rest and re-prime.
                        stats.underrun = true;
                        self.primed = false;
                        self.win = [0.0; 4];
                        self.frac = 0.0;
                        out[i + 1..].fill(0.0);
                        return stats;
                    }
                }
            }
        }
        stats
    }
}

/// 4-point, 3rd-order Hermite interpolation at `t` in [0, 1) between w[1] and w[2].
#[inline]
fn hermite(w: &[f32; 4], t: f32) -> f32 {
    let c0 = w[1];
    let c1 = 0.5 * (w[2] - w[0]);
    let c2 = w[0] - 2.5 * w[1] + 2.0 * w[2] - 0.5 * w[3];
    let c3 = 0.5 * (w[3] - w[0]) + 1.5 * (w[1] - w[2]);
    ((c3 * t + c2) * t + c1) * t + c0
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;

    /// Simulate a producer running 150 ppm fast and check the controller locks
    /// on without underruns and holds the fill near target.
    #[test]
    fn tracks_clock_drift() {
        let sr = 48_000u32;
        let (mut prod, mut cons) = RingBuffer::<f32>::new(sr as usize);
        let mut rs = DriftResampler::new(sr);
        rs.set_target(960);

        let producer_rate = 48_000.0 * (1.0 + 150e-6);
        let mut produced = 0.0f64;
        let mut n_prod = 0u64;
        let mut out = vec![0.0f32; 480];
        let mut underruns = 0;
        // 120 s of simulated time in 10 ms ticks.
        for tick in 0..12_000u64 {
            let t = (tick + 1) as f64 * 0.01;
            // Capture delivers in 10 ms bursts at its own rate.
            let due = (t * producer_rate) as u64;
            while n_prod < due {
                let x = (2.0 * std::f64::consts::PI * 440.0 * n_prod as f64 / 48_000.0).sin();
                let _ = prod.push(x as f32 * 0.5);
                n_prod += 1;
            }
            produced = n_prod as f64;
            let s = rs.render(&mut cons, &mut out);
            if s.underrun && tick > 10 {
                underruns += 1;
            }
            assert_eq!(s.dropped, 0);
        }
        let _ = produced;
        assert_eq!(underruns, 0);
        assert!((rs.ratio_ppm() - 150.0).abs() < 20.0, "ppm {}", rs.ratio_ppm());
        assert!((rs.fill_avg() - 960.0).abs() < 100.0, "fill {}", rs.fill_avg());
    }
}
