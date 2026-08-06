# statime-embassy-net

PTP ordinary-clock runner for `statime` on timestamp-capable `embassy-net`
Ethernet drivers.

This crate connects:

- `statime` for the PTP protocol and servo,
- `embassy-net` for UDP multicast transport,
- a `statime::Clock` implementation controlling the same hardware clock used
  for packet timestamps.

The network driver must provide packet timestamps through `embassy-net` packet
metadata and asynchronous transmit timestamp polling. The optional `stm32`
feature provides a clock adapter for the Embassy STM32 Ethernet PTP clock.
Applications using it must select their concrete `embassy-stm32` chip feature.
Enable the `defmt` feature for embedded logging.

The runner is currently a single-port UDP/IPv4 ordinary clock using E2E delay
measurement. It is slave-only by default.

See [`src/bin/ptp.rs`](src/bin/ptp.rs) for a complete STM32H743 Embassy
example.
