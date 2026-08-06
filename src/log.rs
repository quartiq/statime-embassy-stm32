use embassy_time::{Duration, Instant};
use statime::{filters::FilterEstimate, observability::port::PortState};

pub struct ServoLog {
    next_log: Instant,
    period: Duration,
}

impl ServoLog {
    pub fn new(period: Duration) -> Self {
        Self {
            next_log: Instant::now() + period,
            period,
        }
    }

    pub fn next_deadline(&self) -> Instant {
        self.next_log
    }

    pub fn log_if_due(&mut self, state: PortState, estimate: Option<FilterEstimate>) {
        let now = Instant::now();
        if now < self.next_log {
            return;
        }
        self.next_log = now + self.period;
        if let Some(estimate) = estimate {
            debug!(
                "ptp: servo state={} offset_ns={=i64} mean_delay_ns={=i64}",
                state_name(state),
                saturating_i64(estimate.offset_from_master.nanos_rounded()),
                saturating_i64(estimate.mean_delay.nanos_rounded()),
            );
        } else {
            debug!(
                "ptp: servo state={} offset_ns=none mean_delay_ns=none",
                state_name(state),
            );
        }
    }
}

fn saturating_i64(value: i128) -> i64 {
    value.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

pub fn state_name(state: PortState) -> &'static str {
    match state {
        PortState::Initializing => "initializing",
        PortState::Faulty => "faulty",
        PortState::Disabled => "disabled",
        PortState::Listening => "listening",
        PortState::PreMaster => "pre_master",
        PortState::Master => "master",
        PortState::Passive => "passive",
        PortState::Uncalibrated => "uncalibrated",
        PortState::Slave => "slave",
    }
}
