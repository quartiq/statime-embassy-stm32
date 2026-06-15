use core::convert::Infallible;

use defmt::{debug, info};
use embassy_stm32::eth::{Instance, PtpClock as EthPtpClock};
use statime::{
    Clock as StatimeClock,
    config::TimePropertiesDS,
    time::{Duration, Time},
};

#[derive(Debug)]
pub struct PtpClock<T: Instance> {
    inner: EthPtpClock<T>,
}

impl<T: Instance> PtpClock<T> {
    pub fn new(inner: EthPtpClock<T>) -> Self {
        info!(
            "ptp: clock increment={=u8}ns addend={=u32:#010x}",
            inner.subsecond_increment().nanos(),
            inner.nominal_addend(),
        );
        Self { inner }
    }

    pub fn inner(&self) -> &EthPtpClock<T> {
        &self.inner
    }
}

impl<T: Instance> StatimeClock for &mut PtpClock<T> {
    type Error = Infallible;

    fn now(&self) -> Time {
        let timestamp = self.inner.now();
        Time::from_nanos(timestamp.seconds as u64 * 1_000_000_000 + timestamp.nanos as u64)
    }

    fn step_clock(&mut self, offset: Duration) -> Result<Time, Self::Error> {
        let nanos = offset
            .nanos_rounded()
            .clamp(i64::MIN as i128, i64::MAX as i128) as i64;
        self.inner.offset_time(nanos);
        debug!("ptp: clock step offset_ns={=i64}", nanos);
        Ok(StatimeClock::now(self))
    }

    fn set_frequency(&mut self, ppm: f64) -> Result<Time, Self::Error> {
        let mut addend = self.inner.nominal_addend();
        if ppm.is_finite() {
            let delta =
                (f64::from(addend) * ppm.abs() * 1e-6 + 0.5).min(f64::from(u32::MAX)) as u32;
            addend = if ppm.is_sign_positive() {
                addend.saturating_add(delta)
            } else {
                addend.saturating_sub(delta).max(1)
            };
        }
        self.inner.set_addend(addend);
        debug!(
            "ptp: clock frequency ppm={=f64} addend={=u32:#010x}",
            ppm, addend
        );
        Ok(StatimeClock::now(self))
    }

    fn set_properties(
        &mut self,
        _time_properties_ds: &TimePropertiesDS,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}
