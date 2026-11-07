//! Minimal HID++ client for Logitech mice: discovery (USB, receivers,
//! Bluetooth) plus DPI through HID++ 2.0 feature 0x2201 (Adjustable DPI, as
//! in libratbag's src/hidpp20.c) or 0x2202 (Extended Adjustable DPI, as in
//! Solaar's settings_templates.py), or HID++ 1.0 register 0x63 (Current
//! Resolution, as in libratbag's src/hidpp10.c) for the older models listed
//! in `HIDPP10_MICE`.

use hidapi::{HidApi, HidDevice};
use std::ffi::CString;
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const VID: u16 = 0x046d;

/// Vendor usage pages carrying HID++ reports: 0xff00 on USB and receivers,
/// 0xff43 on Bluetooth.
const HIDPP_USAGE_PAGES: [u16; 2] = [0xff00, 0xff43];

const REPORT_SHORT: u8 = 0x10; // 7 bytes
const REPORT_LONG: u8 = 0x11; // 20 bytes
const LONG_LEN: usize = 20;
/// The device itself (wired, Bluetooth, or a receiver child node made by
/// the kernel's logitech-dj driver, which translates the index for us).
const DIRECT_IDX: u8 = 0xff;
/// Slots on a receiver we talk to without logitech-dj.
const RECEIVER_SLOTS: std::ops::RangeInclusive<u8> = 1..=6;
/// Low nibble of the function byte; echoed back so we can match replies.
const SW_ID: u8 = 0x08;
const ERROR_MSG_10: u8 = 0x8f;
const ERROR_MSG_20: u8 = 0xff;

const FEATURE_ROOT_IDX: u8 = 0x00;
const FEATURE_DEVICE_NAME: u16 = 0x0005;
const FEATURE_ADJUSTABLE_DPI: u16 = 0x2201;
const FEATURE_EXTENDED_DPI: u16 = 0x2202;

// HID++ 1.0 register access (sub ID byte)
const SET_REGISTER: u8 = 0x80;
const GET_REGISTER: u8 = 0x81;
const SET_LONG_REGISTER: u8 = 0x82;
const GET_LONG_REGISTER: u8 = 0x83;
const REG_CURRENT_RESOLUTION: u8 = 0x63;

const TIMEOUT: Duration = Duration::from_millis(400);

/// How an HID++ 1.0 model maps raw register values to DPI.
enum Dpi10 {
    /// Raw value 0x80 + n is the n-th entry.
    List(&'static [u16]),
    /// Raw value n (from 1) is min + step * n, rounded to 25 DPI.
    Range { min: f32, max: f32, step: f32 },
}

struct Model10 {
    pid: u16,
    /// Device index; 0xff unless the mouse sits behind a receiver.
    index: u8,
    name: &'static str,
    /// G9-style short register (one raw byte) instead of the long X/Y register.
    short_reg: bool,
    dpi: Dpi10,
}

/// HID++ 1.0 mice with DPI data in libratbag's data/devices/*.device.
/// These predate self-describing DPI features, so the table is per model.
const HIDPP10_MICE: &[Model10] = &[
    Model10 { pid: 0xc041, index: DIRECT_IDX, name: "Logitech G5", short_reg: false, dpi: Dpi10::List(&[400, 800, 1600, 2000]) },
    Model10 { pid: 0xc049, index: DIRECT_IDX, name: "Logitech G5", short_reg: false, dpi: Dpi10::List(&[400, 800, 1600, 2000]) },
    Model10 {
        pid: 0xc048,
        index: DIRECT_IDX,
        name: "Logitech G9",
        short_reg: true,
        dpi: Dpi10::List(&[200, 400, 600, 800, 1000, 1200, 1400, 1600, 1800, 2000, 2200, 2400, 2600, 2800, 3000, 3200]),
    },
    Model10 { pid: 0xc066, index: DIRECT_IDX, name: "Logitech G9x", short_reg: false, dpi: Dpi10::Range { min: 200.0, max: 5700.0, step: 50.0 } },
    Model10 { pid: 0xc249, index: DIRECT_IDX, name: "Logitech G9x", short_reg: false, dpi: Dpi10::Range { min: 0.0, max: 5700.0, step: 23.53 } },
    Model10 { pid: 0xc068, index: DIRECT_IDX, name: "Logitech G500", short_reg: false, dpi: Dpi10::Range { min: 0.0, max: 5700.0, step: 23.53 } },
    Model10 { pid: 0xc24e, index: DIRECT_IDX, name: "Logitech G500s", short_reg: false, dpi: Dpi10::Range { min: 0.0, max: 8200.0, step: 50.0 } },
    Model10 { pid: 0xc06b, index: DIRECT_IDX, name: "Logitech G700", short_reg: false, dpi: Dpi10::Range { min: 0.0, max: 5700.0, step: 23.53 } },
    Model10 { pid: 0xc07c, index: DIRECT_IDX, name: "Logitech G700s", short_reg: false, dpi: Dpi10::Range { min: 0.0, max: 8200.0, step: 50.0 } },
    // G700 through its own wireless receiver
    Model10 { pid: 0xc531, index: 1, name: "Logitech G700", short_reg: false, dpi: Dpi10::Range { min: 0.0, max: 5700.0, step: 23.53 } },
];

impl Dpi10 {
    /// (raw value, DPI) pairs, as libratbag's hidpp10_build_dpi_table_*().
    fn table(&self) -> Vec<(u8, u16)> {
        match *self {
            Dpi10::List(list) => list.iter().enumerate().map(|(i, &d)| (0x80 + i as u8, d)).collect(),
            Dpi10::Range { min, max, step } => {
                let raw_max = ((max - min) / step) as u32;
                (1..=raw_max.min(0xff))
                    .map(|i| (i as u8, (((min + step * i as f32) / 25.0).round() * 25.0) as u16))
                    .collect()
            }
        }
    }
}

#[derive(Debug)]
pub enum Error {
    /// The device answered with a HID++ error code.
    Hidpp(u8),
    Timeout,
    Io(String),
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Hidpp(code) => write!(f, "HID++ error: {}", error_name(*code)),
            Error::Timeout => f.write_str("Mouse did not answer (asleep or switched off?)"),
            Error::Io(e) => write!(f, "HID I/O error: {e}"),
            Error::Protocol(e) => f.write_str(e),
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DpiInfo {
    pub current: u16,
    /// Every DPI value the sensor accepts, ascending.
    pub values: Vec<u16>,
}

enum DpiFeature {
    /// 0x2201
    Basic,
    /// 0x2202; remembers whether the sensor has a separate Y axis.
    Extended { y_axis: bool },
    /// HID++ 1.0 register 0x63 with the model's (raw, DPI) table.
    Register { short_reg: bool, table: Vec<(u8, u16)> },
}

pub struct Mouse {
    dev: Rc<HidDevice>,
    pub path: String,
    pub index: u8,
    pub name: String,
    feature_idx: u8,
    feature: DpiFeature,
}

/// A hidraw node carrying HID++ reports.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub path: String,
    pub pid: u16,
}

/// HID++ nodes of all connected Logitech devices, sorted and de-duplicated.
pub fn candidate_nodes(api: &mut HidApi) -> Vec<Node> {
    if api.reset_devices().and_then(|_| api.add_devices(VID, 0)).is_err() {
        return Vec::new();
    }
    let mut nodes: Vec<Node> = api
        .device_list()
        .filter(|d| HIDPP_USAGE_PAGES.contains(&d.usage_page()))
        .map(|d| Node { path: d.path().to_string_lossy().into_owned(), pid: d.product_id() })
        .collect();
    nodes.sort_by(|a, b| a.path.cmp(&b.path));
    nodes.dedup_by(|a, b| a.path == b.path);
    nodes
}

/// Opens every candidate node and returns the mice with controllable DPI,
/// plus a message for each node that could not be opened.
pub fn discover(api: &HidApi, nodes: &[Node]) -> (Vec<Mouse>, Vec<String>) {
    let mut mice = Vec::new();
    let mut errors = Vec::new();
    for Node { path, pid } in nodes {
        let dev = match CString::new(path.as_str()).map(|p| api.open_path(&p)) {
            Ok(Ok(dev)) => Rc::new(dev),
            Ok(Err(e)) => {
                errors.push(format!("Cannot open {path}: {e} (udev rule installed?)"));
                continue;
            }
            Err(_) => continue,
        };
        if let Some(model) = HIDPP10_MICE.iter().find(|m| m.pid == *pid) {
            match Mouse::probe10(&dev, path, model) {
                Ok(m) => mice.push(m),
                Err(e) => errors.push(format!("{}: {e}", model.name)),
            }
            continue;
        }
        match Mouse::probe(&dev, path, DIRECT_IDX) {
            Ok(Some(m)) => mice.push(m),
            Ok(None) => {} // HID++ 2.0 device without DPI, e.g. a keyboard
            // A receiver (or a HID++ 1.0 device) refused the HID++ 2.0 request.
            // With logitech-dj bound, its paired devices have their own nodes.
            Err(Error::Hidpp(_)) if driver_name(path).as_deref() != Some("logitech-djreceiver") => {
                for idx in RECEIVER_SLOTS {
                    if let Ok(Some(m)) = Mouse::probe(&dev, path, idx) {
                        mice.push(m);
                    }
                }
            }
            Err(_) => {}
        }
    }
    (mice, errors)
}

/// Kernel driver bound to a /dev/hidrawN node, e.g. "hid-generic".
fn driver_name(path: &str) -> Option<String> {
    let node = path.rsplit('/').next()?;
    let link = std::fs::read_link(format!("/sys/class/hidraw/{node}/device/driver")).ok()?;
    Some(link.file_name()?.to_string_lossy().into_owned())
}

impl Mouse {
    /// Returns `Ok(None)` for a HID++ 2.0 device that has no DPI feature.
    fn probe(dev: &Rc<HidDevice>, path: &str, index: u8) -> Result<Option<Self>> {
        let mut feature_idx = root_get_feature(dev, index, FEATURE_ADJUSTABLE_DPI)?;
        let mut feature = DpiFeature::Basic;
        if feature_idx == 0 {
            feature_idx = root_get_feature(dev, index, FEATURE_EXTENDED_DPI)?;
            if feature_idx == 0 {
                return Ok(None);
            }
            // getCapabilities: bit 0 of byte 2 = separate Y axis
            let caps = call(dev, index, feature_idx, 0x1, &[0])?;
            feature = DpiFeature::Extended { y_axis: caps[2] & 0x01 != 0 };
        }
        // getSensorCount is function 0 in both features
        if call(dev, index, feature_idx, 0x0, &[])?[0] == 0 {
            return Ok(None);
        }
        let name = read_name(dev, index).unwrap_or_else(|| "Logitech mouse".into());
        Ok(Some(Mouse { dev: dev.clone(), path: path.into(), index, name, feature_idx, feature }))
    }

    /// HID++ 1.0 mice can't describe themselves; trust the table and check
    /// that the resolution register answers.
    fn probe10(dev: &Rc<HidDevice>, path: &str, model: &Model10) -> Result<Self> {
        let mouse = Mouse {
            dev: dev.clone(),
            path: path.into(),
            index: model.index,
            name: model.name.into(),
            feature_idx: 0,
            feature: DpiFeature::Register { short_reg: model.short_reg, table: model.dpi.table() },
        };
        mouse.read_current()?;
        Ok(mouse)
    }

    /// Stable identity across rescans.
    pub fn key(&self) -> (String, u8) {
        (self.path.clone(), self.index)
    }

    pub fn read_info(&self) -> Result<DpiInfo> {
        let mut stream = Vec::new();
        match &self.feature {
            DpiFeature::Register { table, .. } => {
                let mut values: Vec<u16> = table.iter().map(|&(_, d)| d).collect();
                values.sort_unstable();
                values.dedup();
                return Ok(DpiInfo { current: self.read_current()?.0, values });
            }
            // getSensorDpiList: [sensor, values...]
            DpiFeature::Basic => stream.extend_from_slice(&self.call(0x1, &[0])?[1..]),
            // getSensorDpiRanges(sensor, direction, page): [sensor, dir, page, values...],
            // the byte stream continues across pages until it ends in 0x0000.
            DpiFeature::Extended { .. } => {
                for page in 0..32u8 {
                    stream.extend_from_slice(&self.call(0x2, &[0, 0, page])?[3..]);
                    if stream.ends_with(&[0, 0]) {
                        break;
                    }
                }
            }
        }
        let values = parse_dpi_list(&stream);
        if values.is_empty() {
            return Err(Error::Protocol("Mouse returned an empty DPI list".into()));
        }
        Ok(DpiInfo { current: self.read_current()?.0, values })
    }

    /// Returns (current DPI, level-of-detail byte for 0x2202).
    pub fn read_current(&self) -> Result<(u16, u8)> {
        // 0x2201 getSensorDpi: [sensor, dpi, default]
        // 0x2202 getSensorDpiParameters: [sensor, x, default_x, y, default_y, lod]
        // Register 0x63: raw X value in byte 0 (long register: Y in byte 2).
        let (func, lod_at) = match &self.feature {
            DpiFeature::Basic => (0x2, None),
            DpiFeature::Extended { .. } => (0x5, Some(9)),
            DpiFeature::Register { short_reg, table } => {
                let sub = if *short_reg { GET_REGISTER } else { GET_LONG_REGISTER };
                let raw = request(&self.dev, self.index, sub, REG_CURRENT_RESOLUTION, &[], false)?[0];
                let dpi = table.iter().find(|&&(r, _)| r == raw).map(|&(_, d)| d);
                return dpi
                    .map(|d| (d, 0))
                    .ok_or_else(|| Error::Protocol(format!("Unknown raw resolution 0x{raw:02x}")));
            }
        };
        let p = self.call(func, &[0])?;
        let mut dpi = be16(&p[1..]);
        if dpi == 0 {
            dpi = be16(&p[3..]);
        }
        Ok((dpi, lod_at.map_or(0, |i| p[i])))
    }

    pub fn set_dpi(&self, dpi: u16) -> Result<()> {
        let [hi, lo] = dpi.to_be_bytes();
        match &self.feature {
            DpiFeature::Register { short_reg, table } => {
                let Some(&(raw, _)) = table.iter().min_by_key(|&&(_, d)| d.abs_diff(dpi)) else {
                    return Err(Error::Protocol("Empty DPI table".into()));
                };
                if *short_reg {
                    request(&self.dev, self.index, SET_REGISTER, REG_CURRENT_RESOLUTION, &[raw], false)?;
                } else {
                    // Little-endian u16 raw X then Y.
                    request(&self.dev, self.index, SET_LONG_REGISTER, REG_CURRENT_RESOLUTION, &[raw, 0, raw, 0], true)?;
                }
            }
            DpiFeature::Basic => {
                let p = self.call(0x3, &[0, hi, lo])?;
                // Feature version 0 doesn't echo the value; later versions do.
                let echoed = be16(&p[1..]);
                if echoed != 0 && echoed != dpi {
                    return Err(Error::Protocol(format!("Mouse acknowledged {echoed} DPI instead of {dpi}")));
                }
            }
            &DpiFeature::Extended { y_axis } => {
                let (_, lod) = self.read_current()?;
                let (yh, yl) = if y_axis { (hi, lo) } else { (0, 0) };
                self.call(0x6, &[0, hi, lo, yh, yl, lod])?;
            }
        }
        Ok(())
    }

    fn call(&self, func: u8, params: &[u8]) -> Result<[u8; 16]> {
        call(&self.dev, self.index, self.feature_idx, func, params)
    }
}

/// Root feature (index 0), function 0: map a feature ID to its index (0 = absent).
fn root_get_feature(dev: &HidDevice, index: u8, feature: u16) -> Result<u8> {
    let [hi, lo] = feature.to_be_bytes();
    Ok(call(dev, index, FEATURE_ROOT_IDX, 0x0, &[hi, lo])?[0])
}

/// Feature 0x0005: getDeviceNameCount (fn 0), then getDeviceName(offset) (fn 1).
fn read_name(dev: &HidDevice, index: u8) -> Option<String> {
    let idx = root_get_feature(dev, index, FEATURE_DEVICE_NAME).ok().filter(|&i| i != 0)?;
    let len = call(dev, index, idx, 0x0, &[]).ok()?[0] as usize;
    let mut name = Vec::with_capacity(len);
    while name.len() < len {
        let chunk = call(dev, index, idx, 0x1, &[name.len() as u8]).ok()?;
        let take = (len - name.len()).min(chunk.len());
        name.extend_from_slice(&chunk[..take]);
    }
    let name = String::from_utf8_lossy(&name).trim_end_matches('\0').trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Big-endian u16 values ending at 0x0000. A value of the form 0b111x_xxxx…
/// (above 0xe000) is a step size: values continue from the previous one in
/// that step up to the value that follows it.
fn parse_dpi_list(bytes: &[u8]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    let words: Vec<u16> = bytes.chunks_exact(2).map(be16).take_while(|&v| v != 0).collect();
    let mut i = 0;
    while i < words.len() {
        let v = words[i];
        if v >> 13 == 0b111 {
            let step = (v & 0x1fff) as usize;
            if let (Some(&last), Some(&end)) = (out.last(), words.get(i + 1)) {
                if step > 0 && end > last {
                    out.extend((last as usize + step..=end as usize).step_by(step).map(|d| d as u16));
                }
            }
            i += 2;
        } else {
            out.push(v);
            i += 1;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// HID++ 2.0 call: function `func` of the feature at `feature_idx`.
fn call(dev: &HidDevice, index: u8, feature_idx: u8, func: u8, params: &[u8]) -> Result<[u8; 16]> {
    request(dev, index, feature_idx, (func << 4) | SW_ID, params, params.len() > 3)
}

/// Sends one HID++ message (2.0: feature index + function/SW ID; 1.0:
/// register sub ID + register) and waits for the matching reply.
/// Returns the 16 parameter bytes of the reply (short replies zero-padded).
fn request(dev: &HidDevice, index: u8, sub_id: u8, addr: u8, params: &[u8], long: bool) -> Result<[u8; 16]> {
    let mut req = [0u8; LONG_LEN];
    req[1] = index;
    req[2] = sub_id;
    req[3] = addr;
    req[4..4 + params.len()].copy_from_slice(params);
    let len = if !long {
        req[0] = REPORT_SHORT;
        7
    } else {
        req[0] = REPORT_LONG;
        LONG_LEN
    };
    dev.write(&req[..len]).map_err(|e| Error::Io(e.to_string()))?;

    let deadline = Instant::now() + TIMEOUT;
    let mut buf = [0u8; LONG_LEN];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Error::Timeout);
        }
        let n = dev
            .read_timeout(&mut buf, left.as_millis().max(1) as i32)
            .map_err(|e| Error::Io(e.to_string()))?;
        if n < 7 || (buf[0] != REPORT_SHORT && buf[0] != REPORT_LONG) {
            continue;
        }
        // Replies from other receiver slots share this node.
        if index != DIRECT_IDX && buf[1] != index {
            continue;
        }
        if buf[2] == sub_id && buf[3] == addr {
            let mut out = [0u8; 16];
            out[..n - 4].copy_from_slice(&buf[4..n]);
            return Ok(out);
        }
        if (buf[2] == ERROR_MSG_10 || buf[2] == ERROR_MSG_20) && buf[3] == sub_id && buf[4] == addr {
            return Err(Error::Hidpp(buf[5]));
        }
        // Anything else is an unrelated notification; keep waiting.
    }
}

fn error_name(code: u8) -> String {
    let name = match code {
        0x01 => "unknown",
        0x02 => "invalid argument",
        0x03 => "out of range",
        0x04 => "hardware error",
        0x05 => "logitech internal",
        0x06 => "invalid feature index",
        0x07 => "invalid function",
        0x08 => "busy",
        0x09 => "unsupported",
        _ => return format!("code 0x{code:02x}"),
    };
    format!("{name} (0x{code:02x})")
}

#[cfg(test)]
mod tests {
    use super::{parse_dpi_list, Dpi10, HIDPP10_MICE};

    fn bytes(words: &[u16]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    #[test]
    fn single_range() {
        // G402: 252, step 84, up to 4032
        let v = parse_dpi_list(&bytes(&[252, 0xe000 + 84, 4032, 0]));
        assert_eq!((v[0], v[1], *v.last().unwrap(), v.len()), (252, 336, 4032, 46));
    }

    #[test]
    fn mixed_ranges_and_list() {
        let v = parse_dpi_list(&bytes(&[100, 0xe000 + 50, 1000, 0xe000 + 100, 1200, 1600, 0, 9999]));
        assert_eq!(v.len(), 19 + 2 + 1);
        assert_eq!(&v[17..], &[950, 1000, 1100, 1200, 1600]);
    }

    #[test]
    fn hidpp10_tables() {
        let g9 = Dpi10::List(&[200, 400, 3200]).table();
        assert_eq!(g9, vec![(0x80, 200), (0x81, 400), (0x82, 3200)]);

        // G500: 0:5700@23.53 -> raw 1..=242, rounded to 25 DPI
        let g500 = Dpi10::Range { min: 0.0, max: 5700.0, step: 23.53 }.table();
        assert_eq!((g500.len(), g500[0], *g500.last().unwrap()), (242, (1, 25), (242, 5700)));

        // G500s: 0:8200@50 -> raw n = 50n DPI
        let g500s = Dpi10::Range { min: 0.0, max: 8200.0, step: 50.0 }.table();
        assert_eq!((g500s.len(), g500s[0], *g500s.last().unwrap()), (164, (1, 50), (164, 8200)));

        // G9x original starts at 200
        assert_eq!(Dpi10::Range { min: 200.0, max: 5700.0, step: 50.0 }.table()[0], (1, 250));

        for m in HIDPP10_MICE {
            assert!(!m.dpi.table().is_empty(), "{}", m.name);
        }
    }
}
