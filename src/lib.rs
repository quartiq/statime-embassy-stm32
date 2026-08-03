#![no_std]
#![doc = include_str!("../README.md")]

mod clock;
mod log;
mod storage;

use core::{
    future::poll_fn,
    num::NonZero,
    task::{Context, Poll},
};

use defmt::{info, warn};
use embassy_futures::select::select3;
use embassy_net::{
    IpAddress, IpEndpoint, Ipv4Address, Stack, TryError, udp,
    udp::{UdpMetadata, UdpSocket},
};
use embassy_stm32::eth::{Instance, PtpTimestampStore as EthPtpTimestampStore};
use embassy_time::{Duration as EmbassyDuration, Instant, with_deadline};
use rand_core::SeedableRng;
use rand_xorshift::XorShiftRng;
use statime::{
    PtpInstance,
    config::{
        AcceptAnyMaster, ClockIdentity, DelayMechanism, InstanceConfig, PortConfig,
        PtpMinorVersion, TimePropertiesDS, TimeSource,
    },
    filters::{KalmanConfiguration, KalmanFilter},
    port::{NoForwardedTLVs, PortAction, PortActionIterator, TimestampContext},
    time::{Duration, Interval, Time},
};

pub use clock::PtpClock;
pub use embassy_stm32::eth::{PtpTimeProvider, PtpTimestamp};
use log::{ServoLog, state_name};
pub use storage::PtpStorage;

const EVENT_PORT: u16 = 319;
const GENERAL_PORT: u16 = 320;
const PRIMARY_MULTICAST: Ipv4Address = Ipv4Address::new(224, 0, 1, 129);
const LINK_LOCAL_MULTICAST: Ipv4Address = Ipv4Address::new(224, 0, 0, 107);
const TX_TIMESTAMP_TIMEOUT: EmbassyDuration = EmbassyDuration::from_millis(100);
const LOG_PERIOD: EmbassyDuration = EmbassyDuration::from_secs(10);
const TX_PENDING: usize = 4;
const TX_TIMESTAMPS: usize = 8;
const RX_TIMESTAMPS: usize = 8;

pub type PtpTimestampStore = EthPtpTimestampStore<TX_TIMESTAMPS, RX_TIMESTAMPS>;

const MSG_DELAY_REQ: u8 = 0x1;
const MSG_PDELAY_REQ: u8 = 0x2;
const MSG_PDELAY_RESP: u8 = 0x3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    /// Ethernet MAC address used to derive the PTP clock identity.
    pub mac_address: [u8; 6],
    /// Seed for statime's per-port random number generator.
    pub rng_seed: u64,
    /// PTP domain number accepted and transmitted by this ordinary clock.
    pub domain_number: u8,
    /// Best master clock algorithm priority1 value.
    pub priority_1: u8,
    /// Best master clock algorithm priority2 value.
    pub priority_2: u8,
    /// Keep this clock out of master state.
    pub slave_only: bool,
    /// Logarithmic E2E delay request interval.
    pub delay_request_interval: Interval,
    /// Logarithmic announce interval.
    pub announce_interval: Interval,
    /// Logarithmic sync interval.
    pub sync_interval: Interval,
    /// Number of missed announces before announce receipt timeout.
    pub announce_receipt_timeout: u8,
    /// Static path delay asymmetry correction.
    pub delay_asymmetry: Duration,
    /// PTP v2 minor version used by the statime port.
    pub minor_ptp_version: PtpMinorVersion,
    /// Time properties advertised by this clock when it becomes master.
    pub time_properties: TimePropertiesDS,
    /// Maximum time to wait for a hardware transmit timestamp.
    pub tx_timestamp_timeout: EmbassyDuration,
    /// Period for compact servo status logging.
    pub log_period: EmbassyDuration,
}

impl Config {
    pub fn new(mac_address: [u8; 6], rng_seed: u64) -> Self {
        Self {
            mac_address,
            rng_seed,
            domain_number: 0,
            priority_1: 128,
            priority_2: 128,
            slave_only: true,
            delay_request_interval: Interval::from_log_2(0),
            announce_interval: Interval::from_log_2(0),
            sync_interval: Interval::from_log_2(0),
            announce_receipt_timeout: 3,
            delay_asymmetry: Duration::ZERO,
            minor_ptp_version: PtpMinorVersion::Zero,
            time_properties: TimePropertiesDS::new_arbitrary_time(
                false,
                false,
                TimeSource::InternalOscillator,
            ),
            tx_timestamp_timeout: TX_TIMESTAMP_TIMEOUT,
            log_period: LOG_PERIOD,
        }
    }
}

/// Single-port PTP ordinary-clock service.
///
/// Construct one runner per Ethernet port and call [`run`](Self::run) from a
/// background task. Dropping the future is not a supported recovery path; this
/// is intended to run for the lifetime of the network stack.
pub struct Runner<'a, T: Instance> {
    stack: Stack<'a>,
    clock: PtpClock<T>,
    storage: &'a mut PtpStorage,
    timestamps: &'a PtpTimestampStore,
    config: Config,
}

impl<'a, T: Instance> Runner<'a, T> {
    /// Bind the PTP service to an Embassy network stack, PTP clock, socket
    /// storage, timestamp store, and protocol configuration.
    pub fn new(
        stack: Stack<'a>,
        clock: PtpClock<T>,
        storage: &'a mut PtpStorage,
        timestamps: &'a PtpTimestampStore,
        config: Config,
    ) -> Self {
        Self {
            stack,
            clock,
            storage,
            timestamps,
            config,
        }
    }

    /// Run the PTP service forever.
    pub async fn run(&mut self) -> ! {
        let stack = self.stack;
        let clock = &mut self.clock;
        let storage = &mut *self.storage;
        let timestamps = self.timestamps;
        let config = self.config;

        info!("ptp: waiting for network configuration");
        stack.wait_config_up().await;
        match stack.join_multicast_group(PRIMARY_MULTICAST) {
            Ok(()) => info!("ptp: joined primary multicast group"),
            Err(error) => {
                warn!("ptp: failed to join primary multicast group: {}", error)
            }
        }
        match stack.join_multicast_group(LINK_LOCAL_MULTICAST) {
            Ok(()) => info!("ptp: joined link-local multicast group"),
            Err(error) => {
                warn!("ptp: failed to join link-local multicast group: {}", error)
            }
        }

        let mut event_socket = UdpSocket::new(
            stack,
            &mut storage.event.rx_meta,
            &mut storage.event.rx_buffer,
            &mut storage.event.tx_meta,
            &mut storage.event.tx_buffer,
        );
        let mut general_socket = UdpSocket::new(
            stack,
            &mut storage.general.rx_meta,
            &mut storage.general.rx_buffer,
            &mut storage.general.tx_meta,
            &mut storage.general.tx_buffer,
        );
        event_socket.bind(EVENT_PORT).unwrap();
        general_socket.bind(GENERAL_PORT).unwrap();

        let clock_identity = clock_identity_from_mac(config.mac_address);
        info!(
            "ptp: clock identity {=u64:#020x}",
            u64::from_be_bytes(clock_identity.0),
        );

        let instance = PtpInstance::<KalmanFilter>::new(
            InstanceConfig {
                clock_identity,
                priority_1: config.priority_1,
                priority_2: config.priority_2,
                domain_number: config.domain_number,
                sdo_id: Default::default(),
                slave_only: config.slave_only,
                path_trace: false,
            },
            config.time_properties,
        );
        let port = instance.add_port(
            PortConfig {
                acceptable_master_list: AcceptAnyMaster,
                delay_mechanism: DelayMechanism::E2E {
                    interval: config.delay_request_interval,
                },
                announce_interval: config.announce_interval,
                announce_receipt_timeout: config.announce_receipt_timeout,
                sync_interval: config.sync_interval,
                master_only: false,
                delay_asymmetry: config.delay_asymmetry,
                minor_ptp_version: config.minor_ptp_version,
            },
            KalmanConfiguration::default(),
            clock,
            XorShiftRng::seed_from_u64(config.rng_seed),
        );
        let (mut port, actions) = port.end_bmca();

        let mut timers = Timers::default();
        let mut forwarded_tlvs = NoForwardedTLVs;
        let mut packet_id = PacketIdGenerator::new();
        let mut pending_tx = PendingTxQueue::new(config.tx_timestamp_timeout);
        let mut bmca = deadline_from_now(instance.bmca_interval());
        let mut servo_log = ServoLog::new(config.log_period);

        handle_actions(
            actions,
            &event_socket,
            &general_socket,
            &mut timers,
            &mut packet_id,
            &mut pending_tx,
        )
        .await;
        info!("ptp: task started");

        loop {
            if Instant::now() >= bmca {
                bmca = deadline_from_now(instance.bmca_interval());
                let old_state = port.port_ds().port_state;
                let mut bmca_port = port.start_bmca();
                instance.bmca(&mut [&mut bmca_port]);
                let (running_port, actions) = bmca_port.end_bmca();
                port = running_port;
                let new_state = port.port_ds().port_state;
                if new_state != old_state {
                    info!(
                        "ptp: state {} -> {}",
                        state_name(old_state),
                        state_name(new_state)
                    );
                }
                handle_actions(
                    actions,
                    &event_socket,
                    &general_socket,
                    &mut timers,
                    &mut packet_id,
                    &mut pending_tx,
                )
                .await;
                continue;
            }

            let actions = if let Some((context, timestamp)) = pending_tx.poll_timestamp(timestamps)
            {
                port.handle_send_timestamp(context, time_from(timestamp))
            } else if let Some(timer) = timers.take_due() {
                match timer {
                    StatimeTimer::Announce => port.handle_announce_timer(&mut forwarded_tlvs),
                    StatimeTimer::Sync => port.handle_sync_timer(),
                    StatimeTimer::DelayRequest => port.handle_delay_request_timer(),
                    StatimeTimer::AnnounceReceipt => port.handle_announce_receipt_timer(),
                    StatimeTimer::FilterUpdate => port.handle_filter_update_timer(),
                }
            } else {
                match event_socket.try_recv_from(&mut storage.packet) {
                    Ok((n, meta)) => {
                        let packet = &storage.packet[..n];
                        if let Some(timestamp) = rx_event_timestamp(packet, meta.meta, timestamps) {
                            port.handle_event_receive(packet, time_from(timestamp))
                        } else {
                            PortActionIterator::empty()
                        }
                    }
                    Err(TryError::Other(udp::RecvError::Truncated)) => {
                        warn!("ptp: truncated event packet");
                        PortActionIterator::empty()
                    }
                    Err(TryError::WouldBlock) => {
                        match general_socket.try_recv_from(&mut storage.packet) {
                            Ok((n, _meta)) => {
                                let packet = &storage.packet[..n];
                                if ptp_message_type(packet).is_some() {
                                    port.handle_general_receive(packet)
                                } else {
                                    PortActionIterator::empty()
                                }
                            }
                            Err(TryError::Other(udp::RecvError::Truncated)) => {
                                warn!("ptp: truncated general packet");
                                PortActionIterator::empty()
                            }
                            Err(TryError::WouldBlock) => PortActionIterator::empty(),
                        }
                    }
                }
            };

            handle_actions(
                actions,
                &event_socket,
                &general_socket,
                &mut timers,
                &mut packet_id,
                &mut pending_tx,
            )
            .await;

            servo_log.log_if_due(
                port.port_ds().port_state,
                port.port_current_ds_contribution(),
            );
            let mut next = bmca.min(servo_log.next_deadline());
            if let Some(timer) = timers.next_deadline() {
                next = next.min(timer);
            }
            if let Some(timeout) = pending_tx.next_timeout_deadline() {
                next = next.min(timeout);
            }
            let _ = with_deadline(
                next,
                select3(
                    event_socket.wait_recv_ready(),
                    general_socket.wait_recv_ready(),
                    poll_fn(|cx| pending_tx.poll_timestamp_ready(timestamps, cx)),
                ),
            )
            .await;
        }
    }
}

async fn handle_actions(
    actions: PortActionIterator<'_>,
    event_socket: &UdpSocket<'_>,
    general_socket: &UdpSocket<'_>,
    timers: &mut Timers,
    packet_id: &mut PacketIdGenerator,
    pending_tx: &mut PendingTxQueue,
) {
    for action in actions {
        match action {
            PortAction::SendEvent {
                context,
                data,
                link_local,
            } => {
                let metadata = UdpMetadata {
                    endpoint: multicast_endpoint(EVENT_PORT, link_local),
                    meta: packet_id.next(),
                    local_address: None,
                };
                match event_socket.send_to(data, metadata).await {
                    Ok(()) => pending_tx.push(context, metadata.meta),
                    Err(error) => {
                        warn!("ptp: event send failed: {}", &error)
                    }
                }
            }
            PortAction::SendGeneral { data, link_local } => {
                let metadata = UdpMetadata {
                    endpoint: multicast_endpoint(GENERAL_PORT, link_local),
                    meta: udp::PacketMeta::default(),
                    local_address: None,
                };
                if let Err(error) = general_socket.send_to(data, metadata).await {
                    warn!("ptp: general send failed: {}", error);
                }
            }
            PortAction::ResetAnnounceTimer { duration } => {
                timers.announce = Some(deadline_from_now(duration));
            }
            PortAction::ResetSyncTimer { duration } => {
                timers.sync = Some(deadline_from_now(duration));
            }
            PortAction::ResetDelayRequestTimer { duration } => {
                timers.delay_request = Some(deadline_from_now(duration));
            }
            PortAction::ResetAnnounceReceiptTimer { duration } => {
                timers.announce_receipt = Some(deadline_from_now(duration));
            }
            PortAction::ResetFilterUpdateTimer { duration } => {
                timers.filter_update = Some(deadline_from_now(duration));
            }
            PortAction::ForwardTLV { .. } => {}
        }
    }
}

fn rx_event_timestamp(
    packet: &[u8],
    meta: udp::PacketMeta,
    timestamps: &PtpTimestampStore,
) -> Option<PtpTimestamp> {
    let message_type = ptp_message_type(packet)?;
    if matches!(
        message_type,
        MSG_DELAY_REQ | MSG_PDELAY_REQ | MSG_PDELAY_RESP
    ) {
        return None;
    }
    let timestamp = timestamps.rx_timestamp(meta);
    if timestamp.is_none() {
        warn!(
            "ptp: missing rx timestamp packet_id={=u32} message_type={=u8}",
            meta.id, message_type
        );
    }
    timestamp
}

fn ptp_message_type(packet: &[u8]) -> Option<u8> {
    packet.get(..34)?;
    Some(packet[0] & 0x0f)
}

#[derive(Default)]
struct Timers {
    announce: Option<Instant>,
    sync: Option<Instant>,
    delay_request: Option<Instant>,
    announce_receipt: Option<Instant>,
    filter_update: Option<Instant>,
}

impl Timers {
    fn take_due(&mut self) -> Option<StatimeTimer> {
        let now = Instant::now();
        [
            (StatimeTimer::Announce, &mut self.announce),
            (StatimeTimer::Sync, &mut self.sync),
            (StatimeTimer::DelayRequest, &mut self.delay_request),
            (StatimeTimer::AnnounceReceipt, &mut self.announce_receipt),
            (StatimeTimer::FilterUpdate, &mut self.filter_update),
        ]
        .into_iter()
        .find_map(|(timer, deadline)| {
            if deadline.is_some_and(|at| now >= at) {
                *deadline = None;
                Some(timer)
            } else {
                None
            }
        })
    }

    fn next_deadline(&self) -> Option<Instant> {
        [
            self.announce,
            self.sync,
            self.delay_request,
            self.announce_receipt,
            self.filter_update,
        ]
        .into_iter()
        .flatten()
        .reduce(Ord::min)
    }
}

#[derive(Clone, Copy)]
enum StatimeTimer {
    Announce,
    Sync,
    DelayRequest,
    AnnounceReceipt,
    FilterUpdate,
}

struct PendingTx {
    context: TimestampContext,
    meta: udp::PacketMeta,
    started: Instant,
}

struct PendingTxQueue {
    slots: [Option<PendingTx>; TX_PENDING],
    timeout: EmbassyDuration,
}

impl PendingTxQueue {
    fn new(timeout: EmbassyDuration) -> Self {
        Self {
            slots: Default::default(),
            timeout,
        }
    }

    fn push(&mut self, context: TimestampContext, meta: udp::PacketMeta) {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(PendingTx {
                context,
                meta,
                started: Instant::now(),
            });
        } else {
            warn!("ptp: tx timestamp queue full packet_id={=u32}", meta.id);
        }
    }

    fn poll_timestamp(
        &mut self,
        timestamps: &PtpTimestampStore,
    ) -> Option<(TimestampContext, PtpTimestamp)> {
        for slot in self.slots.iter_mut() {
            let Some(pending) = slot else {
                continue;
            };
            if let Some(timestamp) = timestamps.tx_timestamp(pending.meta) {
                let Some(pending) = slot.take() else {
                    continue;
                };
                return Some((pending.context, timestamp));
            }
            if pending.started.elapsed() >= self.timeout {
                warn!(
                    "ptp: missing tx timestamp packet_id={=u32}",
                    pending.meta.id
                );
                *slot = None;
            }
        }
        None
    }

    fn poll_timestamp_ready(
        &self,
        timestamps: &PtpTimestampStore,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        for slot in self.slots.iter() {
            let Some(pending) = slot else {
                continue;
            };
            if timestamps.poll_tx_timestamp(pending.meta, cx).is_ready() {
                return Poll::Ready(());
            }
        }
        Poll::Pending
    }

    fn next_timeout_deadline(&self) -> Option<Instant> {
        self.slots
            .iter()
            .filter_map(|slot| {
                let pending = slot.as_ref()?;
                Some(pending.started + self.timeout)
            })
            .reduce(Ord::min)
    }
}

struct PacketIdGenerator(NonZero<u32>);

impl PacketIdGenerator {
    const fn new() -> Self {
        Self(NonZero::<u32>::MIN)
    }

    fn next(&mut self) -> udp::PacketMeta {
        let id = self.0;
        self.0 = self.0.checked_add(1).unwrap_or(NonZero::<u32>::MIN);
        let mut meta = udp::PacketMeta::default();
        meta.id = id.get();
        meta
    }
}

fn clock_identity_from_mac(mac: [u8; 6]) -> ClockIdentity {
    // Use the IEEE EUI-64 expansion, not statime's zero-padded helper.
    ClockIdentity([mac[0], mac[1], mac[2], 0xff, 0xfe, mac[3], mac[4], mac[5]])
}

fn time_from(timestamp: PtpTimestamp) -> Time {
    Time::from_nanos(timestamp.seconds as u64 * 1_000_000_000 + timestamp.nanos as u64)
}

fn multicast_endpoint(port: u16, link_local: bool) -> IpEndpoint {
    let address = if link_local {
        LINK_LOCAL_MULTICAST
    } else {
        PRIMARY_MULTICAST
    };
    IpEndpoint::new(IpAddress::Ipv4(address), port)
}

fn deadline_from_now(duration: core::time::Duration) -> Instant {
    let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
    Instant::now() + EmbassyDuration::from_nanos(nanos)
}
