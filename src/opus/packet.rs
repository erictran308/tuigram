use crate::opus::Error;

pub(crate) const MAX_FRAMES: usize = 48;
pub(crate) const MAX_PACKET_SIZE: usize = 1500;
pub(crate) const MAX_DURATION_SAMPLES_48K: usize = 5760; // 120 ms @ 48 kHz

#[derive(Debug, Clone, Copy)]
pub(crate) struct ParsedPacket<'a> {
    // Used by CELT/SILK mode selection.
    pub toc: u8,
    pub packet_channels: u8,
    pub frame_count: usize,
    // Used by the actual decoder to feed individual frames into CELT/SILK.
    pub frames: [&'a [u8]; MAX_FRAMES],
    pub samples_per_frame_48k: usize,
}

impl<'a> ParsedPacket<'a> {
    pub fn frames(&self) -> &[&'a [u8]] {
        &self.frames[..self.frame_count]
    }

    pub fn samples_per_channel_48k(&self) -> usize {
        self.samples_per_frame_48k * self.frame_count
    }

    pub fn samples_per_channel(&self, fs_hz: u32) -> usize {
        (self.samples_per_channel_48k() * fs_hz as usize) / 48_000
    }
}

pub(crate) fn parse_packet(packet: &[u8]) -> Result<ParsedPacket<'_>, Error> {
    if packet.is_empty() {
        return Err(Error::BadPacket);
    }
    if packet.len() > MAX_PACKET_SIZE {
        return Err(Error::PacketTooLarge);
    }

    let toc = packet[0];
    let packet_channels = if (toc & 0x04) != 0 { 2 } else { 1 };
    let samples_per_frame_48k = samples_per_frame_48k(toc);

    let mut frames: [&[u8]; MAX_FRAMES] = [&[]; MAX_FRAMES];

    let code = toc & 0x03;
    let frame_count = match code {
        0 => {
            frames[0] = &packet[1..];
            1
        }
        1 => {
            let payload = &packet[1..];
            if payload.len() < 2 {
                return Err(Error::BadPacket);
            }
            if !payload.len().is_multiple_of(2) {
                return Err(Error::BadPacket);
            }
            let sz0 = payload.len() / 2;
            frames[0] = &payload[..sz0];
            frames[1] = &payload[sz0..];
            2
        }
        2 => {
            let payload = &packet[1..];
            let (sz0, used) = parse_size(payload)?;
            let payload = &payload[used..];
            if sz0 > payload.len() {
                return Err(Error::BadPacket);
            }
            frames[0] = &payload[..sz0];
            frames[1] = &payload[sz0..];
            2
        }
        3 => {
            let payload = &packet[1..];
            if payload.is_empty() {
                return Err(Error::BadPacket);
            }

            let ch = payload[0];
            let frame_count = (ch & 0x3f) as usize;
            if !(1..=MAX_FRAMES).contains(&frame_count) {
                return Err(Error::BadPacket);
            }

            // RFC 6716: in the code-3 "frame count" byte, bit 6 indicates padding
            // and bit 7 indicates VBR.
            let has_padding = (ch & 0x40) != 0;
            let vbr = (ch & 0x80) != 0;

            let mut idx = 1usize; // into payload
            let mut data_end = payload.len(); // exclusive, relative to payload

            if has_padding {
                let (pad_len, used) = parse_padding_len(&payload[idx..])?;
                idx += used;
                if pad_len > payload.len().saturating_sub(idx) {
                    return Err(Error::BadPacket);
                }
                data_end = payload.len() - pad_len;
                if idx > data_end {
                    return Err(Error::BadPacket);
                }
            }

            let mut sizes = [0usize; MAX_FRAMES];
            if vbr {
                let mut sum = 0usize;
                for s in sizes.iter_mut().take(frame_count - 1) {
                    let (sz, used) = parse_size(&payload[idx..data_end])?;
                    idx += used;
                    *s = sz;
                    sum = sum.saturating_add(sz);
                }
                let remaining = data_end.saturating_sub(idx);
                if sum > remaining {
                    return Err(Error::BadPacket);
                }
                sizes[frame_count - 1] = remaining - sum;
            } else {
                let remaining = data_end.saturating_sub(idx);
                if !remaining.is_multiple_of(frame_count) {
                    return Err(Error::BadPacket);
                }
                let sz = remaining / frame_count;
                for s in &mut sizes[..frame_count] {
                    *s = sz;
                }
            }

            // Now slice out each frame from the payload.
            let mut off = idx;
            for i in 0..frame_count {
                let sz = sizes[i];
                if off + sz > data_end {
                    return Err(Error::BadPacket);
                }
                frames[i] = &payload[off..off + sz];
                off += sz;
            }
            if off != data_end {
                return Err(Error::BadPacket);
            }

            frame_count
        }
        _ => return Err(Error::BadPacket),
    };

    let total_samples_48k = samples_per_frame_48k.saturating_mul(frame_count);
    if total_samples_48k > MAX_DURATION_SAMPLES_48K {
        return Err(Error::BadPacket);
    }

    Ok(ParsedPacket {
        toc,
        packet_channels,
        frame_count,
        frames,
        samples_per_frame_48k,
    })
}

fn parse_size(data: &[u8]) -> Result<(usize, usize), Error> {
    if data.is_empty() {
        return Err(Error::BadPacket);
    }
    let b0 = data[0] as usize;
    if b0 < 252 {
        Ok((b0, 1))
    } else {
        if data.len() < 2 {
            return Err(Error::BadPacket);
        }
        let b1 = data[1] as usize;
        Ok(((b1 << 2) + b0, 2))
    }
}

fn parse_padding_len(data: &[u8]) -> Result<(usize, usize), Error> {
    let mut pad = 0usize;
    let mut used = 0usize;
    loop {
        if used >= data.len() {
            return Err(Error::BadPacket);
        }
        let p = data[used] as usize;
        used += 1;
        // RFC 6716 code-3 padding: each 255 byte contributes 254 and continues.
        let add = if p == 255 { 254 } else { p };
        pad = pad.saturating_add(add);
        if p != 255 {
            break;
        }
    }
    Ok((pad, used))
}

fn samples_per_frame_48k(toc: u8) -> usize {
    // Port of libopus' `opus_packet_get_samples_per_frame()`, specialized to Fs=48000.
    if (toc & 0x80) != 0 {
        let audiosize = ((toc >> 3) & 0x03) as usize;
        (48_000usize << audiosize) / 400
    } else if (toc & 0x60) == 0x60 {
        if (toc & 0x08) != 0 {
            48_000usize / 50
        } else {
            48_000usize / 100
        }
    } else {
        let audiosize = ((toc >> 3) & 0x03) as usize;
        if audiosize == 3 {
            (48_000usize * 60) / 1000
        } else {
            (48_000usize << audiosize) / 100
        }
    }
}
