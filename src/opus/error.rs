/// Why the decoder rejected a call or a packet.
#[derive(Debug)]
pub(crate) enum Error {
    InvalidSampleRate,
    InvalidChannels,
    PacketTooLarge,
    OutputTooSmall,
    BadPacket,
    NotImplemented,
}
