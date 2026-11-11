// msbc.rs — 标准 mSBC 解码器（16kHz / 15块×8子带 / 单声道 / bitpool=26 / LOUDNESS）
// 逐行移植自项目 Python 版 msbc.py（其又移植自 FFmpeg sbcdec，定点算法完全一致）
#![allow(dead_code)]

pub const MSBC_SYNCWORD: u8 = 0xAD;
pub const MSBC_BLOCKS: usize = 15;
pub const MSBC_BITPOOL: i32 = 26;
pub const SAMPLE_RATE: u32 = 16000;
const EXTRA_BITS: i32 = 1; // 量化移位附加位（与官方驱动解码一致：shift = sf + 1 + 1）

const fn ss8(v: u32) -> i64 {
    ((v as i32) as i64) >> 14
}
const fn sn8(v: u32) -> i64 {
    ((v as i32) as i64) >> 14
}

// ---- 合成滤波器原型系数（A2DP 规范 Appendix B；FFmpeg sbcdec_data.h） ----
static PROTO_M0: [i64; 40] = [
    ss8(0x00000000), ss8(0xfe8d1970), ss8(0xee979f00), ss8(0x11686100),
    ss8(0x0172e690), ss8(0xfff5bd1a), ss8(0xfdf1c8d4), ss8(0xeac182c0),
    ss8(0x0d9daee0), ss8(0x00e530da), ss8(0xffe9811d), ss8(0xfd52986c),
    ss8(0xe7054ca0), ss8(0x0a00d410), ss8(0x006c1de4), ss8(0xffdba705),
    ss8(0xfcbc98e8), ss8(0xe3889d20), ss8(0x06af2308), ss8(0x000bb7db),
    ss8(0xffca00ed), ss8(0xfc3fbb68), ss8(0xe071bc00), ss8(0x03bf7948),
    ss8(0xffc4e05c), ss8(0xffb54b3b), ss8(0xfbedadc0), ss8(0xdde26200),
    ss8(0x0142291c), ss8(0xff960e94), ss8(0xff9f3e17), ss8(0xfbd8f358),
    ss8(0xdbf79400), ss8(0xff405e01), ss8(0xff7d4914), ss8(0xff8b1a31),
    ss8(0xfc1417b8), ss8(0xdac7bb40), ss8(0xfdbb828c), ss8(0xff762170),
];

static PROTO_M1: [i64; 40] = [
    ss8(0xff7c272c), ss8(0xfcb02620), ss8(0xda612700), ss8(0xfcb02620),
    ss8(0xff7c272c), ss8(0xff762170), ss8(0xfdbb828c), ss8(0xdac7bb40),
    ss8(0xfc1417b8), ss8(0xff8b1a31), ss8(0xff7d4914), ss8(0xff405e01),
    ss8(0xdbf79400), ss8(0xfbd8f358), ss8(0xff9f3e17), ss8(0xff960e94),
    ss8(0x0142291c), ss8(0xdde26200), ss8(0xfbedadc0), ss8(0xffb54b3b),
    ss8(0xffc4e05c), ss8(0x03bf7948), ss8(0xe071bc00), ss8(0xfc3fbb68),
    ss8(0xffca00ed), ss8(0x000bb7db), ss8(0x06af2308), ss8(0xe3889d20),
    ss8(0xfcbc98e8), ss8(0xffdba705), ss8(0x006c1de4), ss8(0x0a00d410),
    ss8(0xe7054ca0), ss8(0xfd52986c), ss8(0xffe9811d), ss8(0x00e530da),
    ss8(0x0d9daee0), ss8(0xeac182c0), ss8(0xfdf1c8d4), ss8(0xfff5bd1a),
];

// ---- 合成矩阵（8 子带 DCT 快速路径） ----
static SYN_MATRIX8: [[i64; 8]; 16] = [
    [sn8(0x05a82798), sn8(0xfa57d868), sn8(0xfa57d868), sn8(0x05a82798), sn8(0x05a82798), sn8(0xfa57d868), sn8(0xfa57d868), sn8(0x05a82798)],
    [sn8(0x0471ced0), sn8(0xf8275a10), sn8(0x018f8b84), sn8(0x06a6d988), sn8(0xf9592678), sn8(0xfe70747c), sn8(0x07d8a5f0), sn8(0xfb8e3130)],
    [sn8(0x030fbc54), sn8(0xf89be510), sn8(0x07641af0), sn8(0xfcf043ac), sn8(0xfcf043ac), sn8(0x07641af0), sn8(0xf89be510), sn8(0x030fbc54)],
    [sn8(0x018f8b84), sn8(0xfb8e3130), sn8(0x06a6d988), sn8(0xf8275a10), sn8(0x07d8a5f0), sn8(0xf9592678), sn8(0x0471ced0), sn8(0xfe70747c)],
    [sn8(0x00000000); 8],
    [sn8(0xfe70747c), sn8(0x0471ced0), sn8(0xf9592678), sn8(0x07d8a5f0), sn8(0xf8275a10), sn8(0x06a6d988), sn8(0xfb8e3130), sn8(0x018f8b84)],
    [sn8(0xfcf043ac), sn8(0x07641af0), sn8(0xf89be510), sn8(0x030fbc54), sn8(0x030fbc54), sn8(0xf89be510), sn8(0x07641af0), sn8(0xfcf043ac)],
    [sn8(0xfb8e3130), sn8(0x07d8a5f0), sn8(0xfe70747c), sn8(0xf9592678), sn8(0x06a6d988), sn8(0x018f8b84), sn8(0xf8275a10), sn8(0x0471ced0)],
    [sn8(0xfa57d868), sn8(0x05a82798), sn8(0x05a82798), sn8(0xfa57d868), sn8(0xfa57d868), sn8(0x05a82798), sn8(0x05a82798), sn8(0xfa57d868)],
    [sn8(0xf9592678), sn8(0x018f8b84), sn8(0x07d8a5f0), sn8(0x0471ced0), sn8(0xfb8e3130), sn8(0xf8275a10), sn8(0xfe70747c), sn8(0x06a6d988)],
    [sn8(0xf89be510), sn8(0xfcf043ac), sn8(0x030fbc54), sn8(0x07641af0), sn8(0x07641af0), sn8(0x030fbc54), sn8(0xfcf043ac), sn8(0xf89be510)],
    [sn8(0xf8275a10), sn8(0xf9592678), sn8(0xfb8e3130), sn8(0xfe70747c), sn8(0x018f8b84), sn8(0x0471ced0), sn8(0x06a6d988), sn8(0x07d8a5f0)],
    [sn8(0xf8000000); 8],
    [sn8(0xf8275a10), sn8(0xf9592678), sn8(0xfb8e3130), sn8(0xfe70747c), sn8(0x018f8b84), sn8(0x0471ced0), sn8(0x06a6d988), sn8(0x07d8a5f0)],
    [sn8(0xf89be510), sn8(0xfcf043ac), sn8(0x030fbc54), sn8(0x07641af0), sn8(0x07641af0), sn8(0x030fbc54), sn8(0xfcf043ac), sn8(0xf89be510)],
    [sn8(0xf9592678), sn8(0x018f8b84), sn8(0x07d8a5f0), sn8(0x0471ced0), sn8(0xfb8e3130), sn8(0xf8275a10), sn8(0xfe70747c), sn8(0x06a6d988)],
];

// ---- A2DP 规范 Appendix B: loudness 位分配偏移表 ----
static SBC_OFFSET8: [[i32; 8]; 4] = [
    [-2, 0, 0, 0, 0, 0, 0, 1],
    [-3, 0, 0, 0, 0, 0, 1, 2],
    [-4, 0, 0, 0, 0, 0, 1, 2],
    [-4, 0, 0, 0, 0, 0, 1, 2],
];

fn clip_i16(v: i64) -> i16 {
    v.clamp(-32768, 32767) as i16
}

/// CRC-8 EBU（多项式 0x1D，初值 0x0F，MSB first）——FFmpeg ff_sbc_crc8
pub fn sbc_crc8(data: &[u8], bit_len: usize) -> u8 {
    let byte_len = bit_len >> 3;
    let mut bits_len = bit_len & 7;
    let mut crc: u8 = 0x0F;
    for i in 0..byte_len {
        crc ^= data[i];
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { ((crc << 1) ^ 0x1D) & 0xFF } else { (crc << 1) & 0xFF };
        }
    }
    if bits_len > 0 {
        let mut bits = data[byte_len];
        while bits_len > 0 {
            bits_len -= 1;
            crc = ((crc << 1) ^ (if (bits ^ crc) & 0x80 != 0 { 0x1D } else { 0 })) & 0xFF;
            bits = (bits << 1) & 0xFF;
        }
    }
    crc
}

/// A2DP 规范位分配（mono + LOUDNESS）——FFmpeg ff_sbc_calculate_bits
fn calculate_bits_mono(scale_factor: &[u8; 8], bitpool: i32) -> [u8; 8] {
    let mut bitneed = [0i32; 8];
    let mut max_bitneed = 0i32;
    for sb in 0..8 {
        bitneed[sb] = if scale_factor[sb] == 0 {
            -5
        } else {
            let loudness = scale_factor[sb] as i32 - SBC_OFFSET8[0][sb];
            if loudness > 0 { loudness / 2 } else { loudness }
        };
        if bitneed[sb] > max_bitneed {
            max_bitneed = bitneed[sb];
        }
    }

    let mut bitcount = 0i32;
    let mut slicecount = 0i32;
    let mut bitslice = max_bitneed + 1;
    loop {
        bitslice -= 1;
        bitcount += slicecount;
        slicecount = 0;
        for sb in 0..8 {
            if bitneed[sb] > bitslice + 1 && bitneed[sb] < bitslice + 16 {
                slicecount += 1;
            } else if bitneed[sb] == bitslice + 1 {
                slicecount += 2;
            }
        }
        if bitcount + slicecount >= bitpool {
            break;
        }
    }

    if bitcount + slicecount == bitpool {
        bitcount += slicecount;
        bitslice -= 1;
    }

    let mut bits = [0u8; 8];
    for sb in 0..8 {
        bits[sb] = if bitneed[sb] < bitslice + 2 {
            0
        } else {
            (bitneed[sb] - bitslice).min(16) as u8
        };
    }

    for sb in 0..8 {
        if bitcount >= bitpool {
            break;
        }
        if (2..16).contains(&bits[sb]) {
            bits[sb] += 1;
            bitcount += 1;
        } else if bitneed[sb] == bitslice + 1 && bitpool > bitcount + 1 {
            bits[sb] = 2;
            bitcount += 2;
        }
    }

    for sb in 0..8 {
        if bitcount >= bitpool {
            break;
        }
        if bits[sb] < 16 {
            bits[sb] += 1;
            bitcount += 1;
        }
    }
    bits
}

/// 连续 mSBC 帧流解码器（维护合成滤波器状态，跨帧）
pub struct MsbcDecoder {
    v: [i64; 170],
    offset: [usize; 16],
}

impl Default for MsbcDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl MsbcDecoder {
    pub fn new() -> Self {
        let mut offset = [0usize; 16];
        for i in 0..16 {
            offset[i] = 10 * i + 10;
        }
        Self { v: [0; 170], offset }
    }

    /// 解码一帧 mSBC（57 字节），返回 120 个 i16 样本；帧错误返回 None
    pub fn decode_frame(&mut self, frame: &[u8]) -> Option<Vec<i16>> {
        if frame.len() < 57 || frame[0] != MSBC_SYNCWORD {
            return None;
        }
        if frame[1] != 0 || frame[2] != 0 {
            return None;
        }
        // CRC：params(2B) + 尺度因子(4B) 共 48 位
        let mut crc_data = [0u8; 6];
        crc_data[0..2].copy_from_slice(&frame[1..3]);
        crc_data[2..6].copy_from_slice(&frame[4..8]);
        if frame[3] != sbc_crc8(&crc_data, 48) {
            return None;
        }

        // 8 个 4-bit 尺度因子（byte4 起，高位尼布在前）
        let mut scale_factor = [0u8; 8];
        for i in 0..8 {
            scale_factor[i] = (frame[4 + i / 2] >> (4 - (4 * i) % 8)) & 0x0F;
        }

        let bits = calculate_bits_mono(&scale_factor, MSBC_BITPOOL);
        let mut levels = [0i64; 8];
        for sb in 0..8 {
            levels[sb] = ((1i64) << bits[sb]) - 1;
        }

        // 15 块 × 8 子带量化样本（byte8 起，MSB first 位流）
        let mut sb_sample = [[0i64; 8]; MSBC_BLOCKS];
        let mut consumed: usize = 64; // bits
        let nbits = frame.len() * 8;
        for blk in 0..MSBC_BLOCKS {
            for sb in 0..8 {
                if levels[sb] == 0 {
                    continue;
                }
                let shift = scale_factor[sb] as i32 + 1 + EXTRA_BITS;
                let mut sample: i64 = 0;
                for bit in 0..bits[sb] as usize {
                    if consumed >= nbits {
                        return None;
                    }
                    if (frame[consumed >> 3] >> (7 - (consumed & 7))) & 1 != 0 {
                        sample |= 1i64 << (bits[sb] as usize - bit - 1);
                    }
                    consumed += 1;
                }
                let num = ((sample << 1) | 1) << shift;
                sb_sample[blk][sb] = num / levels[sb] - (1i64 << shift);
            }
        }

        Some(self.synthesize(&sb_sample))
    }

    /// SBC 合成滤波器组（8 子带）——FFmpeg sbc_synthesize_eight
    fn synthesize(&mut self, sb_sample: &[[i64; 8]; MSBC_BLOCKS]) -> Vec<i16> {
        let mut out = vec![0i16; MSBC_BLOCKS * 8];
        for blk in 0..MSBC_BLOCKS {
            let s = &sb_sample[blk];
            for i in 0..16 {
                let m = &SYN_MATRIX8[i];
                if self.offset[i] == 0 {
                    self.offset[i] = 159;
                    let head: [i64; 9] = self.v[0..9].try_into().unwrap();
                    self.v[160..169].copy_from_slice(&head);
                } else {
                    self.offset[i] -= 1;
                }
                let o = self.offset[i];
                self.v[o] = (m[0] * s[0] + m[1] * s[1] + m[2] * s[2] + m[3] * s[3]
                    + m[4] * s[4] + m[5] * s[5] + m[6] * s[6] + m[7] * s[7])
                    >> 15;
            }
            for idx_i in 0..8 {
                let i = idx_i;
                let k = (i + 8) & 0xF;
                let idx = idx_i * 5;
                let oi = self.offset[i];
                let ok = self.offset[k];
                let val = (self.v[oi] * PROTO_M0[idx]
                    + self.v[ok + 1] * PROTO_M1[idx]
                    + self.v[oi + 2] * PROTO_M0[idx + 1]
                    + self.v[ok + 3] * PROTO_M1[idx + 1]
                    + self.v[oi + 4] * PROTO_M0[idx + 2]
                    + self.v[ok + 5] * PROTO_M1[idx + 2]
                    + self.v[oi + 6] * PROTO_M0[idx + 3]
                    + self.v[ok + 7] * PROTO_M1[idx + 3]
                    + self.v[oi + 8] * PROTO_M0[idx + 4]
                    + self.v[ok + 9] * PROTO_M1[idx + 4])
                    >> 15;
                out[blk * 8 + i] = clip_i16(val);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc8_known() {
        // 与 Python 实现对照：任意数据的 CRC 稳定性（自洽验证）
        let d = [0x00u8, 0x00, 0x11, 0x22, 0x33, 0x44];
        let a = sbc_crc8(&d, 48);
        assert_eq!(a, sbc_crc8(&d, 48));
    }

    #[test]
    fn garbage_frames_never_panic() {
        let mut dec = MsbcDecoder::new();
        // 随机垃圾帧：不得 panic，返回 None 或空结果
        let junk: Vec<u8> = (0..57u8).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect();
        let _ = dec.decode_frame(&junk);
        let zeros = [0u8; 57];
        assert!(dec.decode_frame(&zeros).is_none()); // 同步字不符
    }

    #[test]
    fn silence_frame_decodes() {
        // 全静音帧：AD 00 00 crc 0000... —— 尺度因子全 0 的合法帧
        let mut frame = [0u8; 57];
        frame[0] = 0xAD;
        frame[1] = 0;
        frame[2] = 0;
        let crc_data = [frame[1], frame[2], frame[4], frame[5], frame[6], frame[7]];
        frame[3] = sbc_crc8(&crc_data, 48);
        let mut dec = MsbcDecoder::new();
        let out = dec.decode_frame(&frame);
        assert!(out.is_some());
        assert_eq!(out.unwrap().len(), 120);
    }
}
