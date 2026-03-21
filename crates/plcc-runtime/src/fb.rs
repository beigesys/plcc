// SPDX-License-Identifier: MPL-2.0

//! Standard function blocks per IEC 61131-3 Section 2.5.2.

use crate::traits::FunctionBlock;

// ── Bistable FBs ──

/// SR (Set-dominant) flip-flop.
#[derive(Debug, Clone, Default)]
pub struct SR {
    pub set1: bool,
    pub reset: bool,
    pub q1: bool,
}

impl FunctionBlock for SR {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        self.q1 = self.set1 || (!self.reset && self.q1);
    }
    fn reset(&mut self) {
        self.init();
    }
}

/// RS (Reset-dominant) flip-flop.
#[derive(Debug, Clone, Default)]
pub struct RS {
    pub set: bool,
    pub reset1: bool,
    pub q1: bool,
}

impl FunctionBlock for RS {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        self.q1 = !self.reset1 && (self.set || self.q1);
    }
    fn reset(&mut self) {
        self.init();
    }
}

// ── Edge detection ──

/// R_TRIG: Rising edge detector.
#[derive(Debug, Clone, Default)]
pub struct RTrig {
    pub clk: bool,
    pub q: bool,
    prev: bool,
}

impl FunctionBlock for RTrig {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        self.q = self.clk && !self.prev;
        self.prev = self.clk;
    }
    fn reset(&mut self) {
        self.init();
    }
}

/// F_TRIG: Falling edge detector.
#[derive(Debug, Clone, Default)]
pub struct FTrig {
    pub clk: bool,
    pub q: bool,
    prev: bool,
}

impl FunctionBlock for FTrig {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        self.q = !self.clk && self.prev;
        self.prev = self.clk;
    }
    fn reset(&mut self) {
        self.init();
    }
}

// ── Counters ──

/// CTU: Count Up.
#[derive(Debug, Clone, Default)]
pub struct CTU {
    pub cu: bool,
    pub reset: bool,
    pub pv: i32,
    pub q: bool,
    pub cv: i32,
    prev_cu: bool,
}

impl FunctionBlock for CTU {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        if self.reset {
            self.cv = 0;
        } else if self.cu && !self.prev_cu {
            self.cv = self.cv.saturating_add(1);
        }
        self.prev_cu = self.cu;
        self.q = self.cv >= self.pv;
    }
    fn reset(&mut self) {
        self.init();
    }
}

/// CTD: Count Down.
#[derive(Debug, Clone, Default)]
pub struct CTD {
    pub cd: bool,
    pub load: bool,
    pub pv: i32,
    pub q: bool,
    pub cv: i32,
    prev_cd: bool,
}

impl FunctionBlock for CTD {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        if self.load {
            self.cv = self.pv;
        } else if self.cd && !self.prev_cd {
            self.cv = self.cv.saturating_sub(1);
        }
        self.prev_cd = self.cd;
        self.q = self.cv <= 0;
    }
    fn reset(&mut self) {
        self.init();
    }
}

/// CTUD: Count Up/Down.
#[derive(Debug, Clone, Default)]
pub struct CTUD {
    pub cu: bool,
    pub cd: bool,
    pub reset: bool,
    pub load: bool,
    pub pv: i32,
    pub qu: bool,
    pub qd: bool,
    pub cv: i32,
    prev_cu: bool,
    prev_cd: bool,
}

impl FunctionBlock for CTUD {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        if self.reset {
            self.cv = 0;
        } else if self.load {
            self.cv = self.pv;
        } else {
            if self.cu && !self.prev_cu {
                self.cv = self.cv.saturating_add(1);
            }
            if self.cd && !self.prev_cd {
                self.cv = self.cv.saturating_sub(1);
            }
        }
        self.prev_cu = self.cu;
        self.prev_cd = self.cd;
        self.qu = self.cv >= self.pv;
        self.qd = self.cv <= 0;
    }
    fn reset(&mut self) {
        self.init();
    }
}

// ── Timers ──

/// TON: Timer On-Delay.
/// Requires external elapsed_ms to be provided each scan.
#[derive(Debug, Clone, Default)]
pub struct TON {
    pub input: bool,
    pub pt_ms: u64,
    pub q: bool,
    pub et_ms: u64,
    running: bool,
    elapsed: u64,
}

impl TON {
    pub fn scan_with_time(&mut self, elapsed_ms: u64) {
        if self.input {
            if !self.running {
                self.running = true;
                self.elapsed = 0;
            }
            if !self.q {
                self.elapsed += elapsed_ms;
                if self.elapsed >= self.pt_ms {
                    self.q = true;
                    self.et_ms = self.pt_ms;
                } else {
                    self.et_ms = self.elapsed;
                }
            }
        } else {
            self.running = false;
            self.q = false;
            self.et_ms = 0;
            self.elapsed = 0;
        }
    }
}

impl FunctionBlock for TON {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        // Default: assume 1ms elapsed per scan
        self.scan_with_time(1);
    }
    fn reset(&mut self) {
        self.init();
    }
}

/// TOF: Timer Off-Delay.
#[derive(Debug, Clone, Default)]
pub struct TOF {
    pub input: bool,
    pub pt_ms: u64,
    pub q: bool,
    pub et_ms: u64,
    running: bool,
    elapsed: u64,
}

impl TOF {
    pub fn scan_with_time(&mut self, elapsed_ms: u64) {
        if !self.input {
            if self.running {
                self.elapsed += elapsed_ms;
                if self.elapsed >= self.pt_ms {
                    self.q = false;
                    self.et_ms = self.pt_ms;
                    self.running = false;
                } else {
                    self.et_ms = self.elapsed;
                }
            }
        } else {
            self.q = true;
            self.running = true;
            self.elapsed = 0;
            self.et_ms = 0;
        }
    }
}

impl FunctionBlock for TOF {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        self.scan_with_time(1);
    }
    fn reset(&mut self) {
        self.init();
    }
}

/// TP: Timer Pulse.
#[derive(Debug, Clone, Default)]
pub struct TP {
    pub input: bool,
    pub pt_ms: u64,
    pub q: bool,
    pub et_ms: u64,
    running: bool,
    elapsed: u64,
    prev_input: bool,
}

impl TP {
    pub fn scan_with_time(&mut self, elapsed_ms: u64) {
        if self.input && !self.prev_input && !self.running {
            // Rising edge — start pulse
            self.running = true;
            self.elapsed = 0;
            self.q = true;
        }
        if self.running {
            self.elapsed += elapsed_ms;
            if self.elapsed >= self.pt_ms {
                self.q = false;
                self.et_ms = self.pt_ms;
                self.running = false;
            } else {
                self.et_ms = self.elapsed;
            }
        }
        self.prev_input = self.input;
    }
}

impl FunctionBlock for TP {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        self.scan_with_time(1);
    }
    fn reset(&mut self) {
        self.init();
    }
}

/// RTC: Real-Time Clock (simplified — just passes through).
#[derive(Debug, Clone, Default)]
pub struct RTC {
    pub en: bool,
    pub pdt: i64,
    pub q: bool,
    pub cdt: i64,
}

impl FunctionBlock for RTC {
    fn init(&mut self) {
        *self = Self::default();
    }
    fn scan(&mut self) {
        self.q = self.en;
        if self.en {
            self.cdt = self.pdt;
        }
    }
    fn reset(&mut self) {
        self.init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sr() {
        let mut sr = SR::default();
        sr.set1 = true;
        sr.scan();
        assert!(sr.q1);
        sr.set1 = false;
        sr.scan();
        assert!(sr.q1); // Latched
        sr.reset = true;
        sr.scan();
        assert!(!sr.q1);
    }

    #[test]
    fn test_r_trig() {
        let mut rt = RTrig::default();
        rt.clk = false;
        rt.scan();
        assert!(!rt.q);
        rt.clk = true;
        rt.scan();
        assert!(rt.q); // Rising edge
        rt.scan();
        assert!(!rt.q); // No edge
    }

    #[test]
    fn test_ctu() {
        let mut ctu = CTU::default();
        ctu.pv = 3;
        for _ in 0..3 {
            ctu.cu = true;
            ctu.scan();
            ctu.cu = false;
            ctu.scan();
        }
        assert!(ctu.q);
        assert_eq!(ctu.cv, 3);
    }

    #[test]
    fn test_ton() {
        let mut ton = TON::default();
        ton.pt_ms = 100;
        ton.input = true;
        for _ in 0..99 {
            ton.scan_with_time(1);
        }
        assert!(!ton.q);
        ton.scan_with_time(1);
        assert!(ton.q);
    }
}
