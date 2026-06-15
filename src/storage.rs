use embassy_net::udp::PacketMetadata;

pub const PACKET_BYTES: usize = 256;
pub const EVENT_RX_PACKETS: usize = 4;
pub const EVENT_TX_PACKETS: usize = 2;
pub const GENERAL_RX_PACKETS: usize = 4;
pub const GENERAL_TX_PACKETS: usize = 1;
pub const EVENT_RX_BYTES: usize = PACKET_BYTES * EVENT_RX_PACKETS;
pub const EVENT_TX_BYTES: usize = PACKET_BYTES * EVENT_TX_PACKETS;
pub const GENERAL_RX_BYTES: usize = PACKET_BYTES * GENERAL_RX_PACKETS;
pub const GENERAL_TX_BYTES: usize = PACKET_BYTES * GENERAL_TX_PACKETS;

type EventStorage =
    SocketStorage<EVENT_RX_PACKETS, EVENT_TX_PACKETS, EVENT_RX_BYTES, EVENT_TX_BYTES>;
type GeneralStorage =
    SocketStorage<GENERAL_RX_PACKETS, GENERAL_TX_PACKETS, GENERAL_RX_BYTES, GENERAL_TX_BYTES>;

pub struct PtpStorage {
    pub(super) event: EventStorage,
    pub(super) general: GeneralStorage,
    pub(super) packet: [u8; PACKET_BYTES],
}

impl PtpStorage {
    pub const fn new() -> Self {
        Self {
            event: SocketStorage::new(),
            general: SocketStorage::new(),
            packet: [0; PACKET_BYTES],
        }
    }
}

impl Default for PtpStorage {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) struct SocketStorage<
    const RX_PACKETS: usize,
    const TX_PACKETS: usize,
    const RX_BYTES: usize,
    const TX_BYTES: usize,
> {
    pub(super) rx_meta: [PacketMetadata; RX_PACKETS],
    pub(super) tx_meta: [PacketMetadata; TX_PACKETS],
    pub(super) rx_buffer: [u8; RX_BYTES],
    pub(super) tx_buffer: [u8; TX_BYTES],
}

impl<const RX_PACKETS: usize, const TX_PACKETS: usize, const RX_BYTES: usize, const TX_BYTES: usize>
    SocketStorage<RX_PACKETS, TX_PACKETS, RX_BYTES, TX_BYTES>
{
    const fn new() -> Self {
        Self {
            rx_meta: [PacketMetadata::EMPTY; RX_PACKETS],
            tx_meta: [PacketMetadata::EMPTY; TX_PACKETS],
            rx_buffer: [0; RX_BYTES],
            tx_buffer: [0; TX_BYTES],
        }
    }
}
