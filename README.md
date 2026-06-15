# statime-embassy-stm32

PTP ordinary-clock runner for `statime` on Embassy STM32 Ethernet.

This crate connects:

- `statime` for the PTP protocol and servo,
- `embassy-net` for UDP multicast transport,
- `embassy-stm32` Ethernet PTP hardware timestamps and clock control.

The board crate must
select the concrete `embassy-stm32` chip and time-driver features and must
initialize Ethernet with a PTP timestamp store.

The runner is currently a single-port UDP/IPv4 ordinary clock using E2E delay
measurement. It is slave-only by default.

See [`src/bin/ptp.rs`](src/bin/ptp.rs) for a complete STM32H743 RTIC/Embassy
example.
