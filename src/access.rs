//! One-click permission setup: installs the udev rule that lets the logged-in
//! user open Logitech hidraw nodes, via a single graphical password prompt.

use crate::hidpp::Node;
use std::io::ErrorKind;
use std::process::Command;

pub const RULE: &str = include_str!("../packaging/70-logitech-hidpp.rules");
const RULE_PATH: &str = "/etc/udev/rules.d/70-logitech-hidpp.rules";

/// True if any HID++ node exists but this user may not open it.
pub fn denied(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&n.path)
            .is_err_and(|e| e.kind() == ErrorKind::PermissionDenied)
    })
}

/// Installs the rule (replacing rules from earlier versions) and re-applies
/// udev rules to connected devices. Blocks until the user answers pkexec.
pub fn install_rule() -> Result<(), String> {
    let script = format!(
        "rm -f /etc/udev/rules.d/99-logitech-g402.rules /etc/udev/rules.d/99-logitech-hidpp.rules \
         && printf '%s' \"$1\" > {RULE_PATH} \
         && udevadm control --reload-rules \
         && udevadm trigger --subsystem-match=hidraw \
         && udevadm settle"
    );
    let status = Command::new("pkexec")
        .args(["sh", "-c", &script, "sh", RULE])
        .status()
        .map_err(|e| format!("Cannot run pkexec: {e}"))?;
    match status.code() {
        Some(0) => Ok(()),
        Some(126) => Err("Password prompt was cancelled".into()),
        _ => Err(format!("Installing the udev rule failed ({status})")),
    }
}
