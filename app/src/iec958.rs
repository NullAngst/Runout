//! Read a card's IEC958 (S/PDIF) channel status, when the driver exposes it:
//! the sample rate the stream says it carries and whether it is
//! pre-emphasized. The alsa crate has no accessor for IEC958 values, so this
//! goes through alsa-sys.

use alsa_sys as sys;
use std::ffi::{CStr, CString};
use std::ptr;

pub struct Status {
    pub rate: Option<u32>,
    pub emphasis: bool,
}

/// Channel status as alsa-lib lays it out (asoundef.h, bits LSB first).
/// Consumer: byte 0 bits 3-5 are the emphasis (1 = 50/15 us), byte 3 low
/// nibble is the sample rate.
fn parse(st: &[u8; 24]) -> Status {
    let pro = st[0] & 1 == 1;
    if pro {
        // professional (AES3): emphasis in bits 2-4 of byte 0, 3 = 50/15 us
        let emph = (st[0] >> 2) & 0b111;
        let rate = match (st[0] >> 6) & 0b11 {
            1 => Some(44100),
            2 => Some(48000),
            3 => Some(32000),
            _ => None,
        };
        return Status { rate, emphasis: emph == 3 };
    }
    let rate = match st[3] & 0x0f {
        0x0 => Some(44100),
        0x2 => Some(48000),
        0x3 => Some(32000),
        0xa => Some(96000),
        0x8 => Some(88200),
        0xc => Some(176400),
        0xe => Some(192000),
        _ => None,
    };
    Status { rate, emphasis: (st[0] >> 3) & 0b111 == 1 }
}

/// Status from the first IEC958 capture control on card `card`.
pub fn read_status(card: i32) -> Option<Status> {
    let name = CString::new(format!("hw:{card}")).ok()?;
    // SAFETY: every pointer is checked after its alloc, freed on every path
    // below, and only passed to the alsa-lib calls it was made for.
    unsafe {
        let mut ctl: *mut sys::snd_ctl_t = ptr::null_mut();
        if sys::snd_ctl_open(&mut ctl, name.as_ptr(), 0) < 0 || ctl.is_null() {
            return None;
        }
        let mut list: *mut sys::snd_ctl_elem_list_t = ptr::null_mut();
        let mut id: *mut sys::snd_ctl_elem_id_t = ptr::null_mut();
        let mut info: *mut sys::snd_ctl_elem_info_t = ptr::null_mut();
        let mut val: *mut sys::snd_ctl_elem_value_t = ptr::null_mut();
        let ok = sys::snd_ctl_elem_list_malloc(&mut list) >= 0 && sys::snd_ctl_elem_id_malloc(&mut id) >= 0 && sys::snd_ctl_elem_info_malloc(&mut info) >= 0 && sys::snd_ctl_elem_value_malloc(&mut val) >= 0;
        let mut out = None;
        if ok && sys::snd_ctl_elem_list(ctl, list) >= 0 {
            let count = sys::snd_ctl_elem_list_get_count(list);
            if count > 0 && sys::snd_ctl_elem_list_alloc_space(list, count) >= 0 && sys::snd_ctl_elem_list(ctl, list) >= 0 {
                for i in 0..count {
                    let n = sys::snd_ctl_elem_list_get_name(list, i);
                    if n.is_null() {
                        continue;
                    }
                    let nm = CStr::from_ptr(n).to_string_lossy().to_lowercase();
                    if !nm.contains("iec958") || nm.contains("mask") || nm.contains("switch") || nm.contains("playback") {
                        continue;
                    }
                    if !(nm.contains("capture") || nm.contains(" in") || nm.contains("input")) {
                        continue;
                    }
                    sys::snd_ctl_elem_list_get_id(list, i, id);
                    sys::snd_ctl_elem_info_set_id(info, id);
                    if sys::snd_ctl_elem_info(ctl, info) < 0 || sys::snd_ctl_elem_info_get_type(info) != sys::SND_CTL_ELEM_TYPE_IEC958 {
                        continue;
                    }
                    sys::snd_ctl_elem_value_set_id(val, id);
                    if sys::snd_ctl_elem_read(ctl, val) < 0 {
                        continue;
                    }
                    let mut aes: sys::snd_aes_iec958_t = std::mem::zeroed();
                    sys::snd_ctl_elem_value_get_iec958(val, &mut aes);
                    out = Some(parse(&aes.status));
                    break;
                }
                sys::snd_ctl_elem_list_free_space(list);
            }
        }
        if !val.is_null() {
            sys::snd_ctl_elem_value_free(val);
        }
        if !info.is_null() {
            sys::snd_ctl_elem_info_free(info);
        }
        if !id.is_null() {
            sys::snd_ctl_elem_id_free(id);
        }
        if !list.is_null() {
            sys::snd_ctl_elem_list_free(list);
        }
        sys::snd_ctl_close(ctl);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumer_status() {
        let mut s = [0u8; 24];
        s[3] = 0x2;
        let st = parse(&s);
        assert_eq!(st.rate, Some(48000));
        assert!(!st.emphasis);
        s[0] = 0b0000_1000;
        assert!(parse(&s).emphasis);
        s[0] = 0b0001_0000;
        assert!(!parse(&s).emphasis);
    }

    #[test]
    fn professional_status() {
        let mut s = [0u8; 24];
        s[0] = 1 | (3 << 2) | (2 << 6);
        let st = parse(&s);
        assert_eq!(st.rate, Some(48000));
        assert!(st.emphasis);
    }
}
