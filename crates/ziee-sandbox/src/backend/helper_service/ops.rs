//! The privileged operations the helper performs as LocalSystem. Kept
//! deliberately small and free of untrusted input — this is SYSTEM-level
//! code, so the attack surface is the whole point to minimize.

use super::hvsocket;

const GCS_KEY: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Virtualization\GuestCommunicationServices";

/// Resolve the WSL utility VM's VmId. Runs as SYSTEM (in the service) so the
/// `hcsdiag`/HCS call succeeds without the caller being in Hyper-V
/// Administrators. Returns canonical GUID string for the wire.
pub fn resolve_vm_id() -> Result<String, String> {
    let g = hvsocket::wsl_utility_vm_id().map_err(|e| e.to_string())?;
    Ok(hvsocket::fmt_guid(&g))
}

/// Register the `HV_GUID_VSOCK_TEMPLATE` GUIDs for `port_start..+count` under
/// HKLM `GuestCommunicationServices`. Idempotent. Returns the count of newly
/// created entries (0 = all already present). HKLM write needs Administrator
/// — satisfied because the service runs as LocalSystem.
///
/// NOTE: a non-zero return means vmcompute must re-read the list, which only
/// happens on VM (re)start — callers register at install time and pair this
/// with [`wsl_shutdown`]. We deliberately do NOT shut WSL down here at
/// runtime: that would kill every running distro (incl. Docker Desktop)
/// mid-session.
pub fn ensure_registered(port_start: u32, count: u32) -> Result<u32, String> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    // Bound the input — this is the only attacker-influenceable parameter and
    // it comes from our own server, but defense in depth: cap the range so a
    // bug/compromise can't spray thousands of registry keys.
    if count == 0 || count > 1024 {
        return Err(format!("refusing to register {count} ports (allowed 1..=1024)"));
    }

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let gcs = hklm
        .open_subkey_with_flags(GCS_KEY, KEY_READ | KEY_WRITE)
        .map_err(|e| format!("open {GCS_KEY}: {e} (is Hyper-V/WSL2 installed?)"))?;

    let mut added = 0u32;
    for p in port_start..port_start.saturating_add(count) {
        // HV_GUID_VSOCK_TEMPLATE with data1 = port number.
        let guid = format!("{p:08x}-facb-11e6-bd58-64006a7986d3");
        // create_subkey is open-or-create; treat "already had it" by probing
        // first so we can report an accurate `added` count.
        let existed = gcs.open_subkey(&guid).is_ok();
        let (key, _disp) = gcs
            .create_subkey(&guid)
            .map_err(|e| format!("create {guid}: {e}"))?;
        if !existed {
            key.set_value("ElementName", &format!("ziee-sandbox-vsock-{p}"))
                .map_err(|e| format!("set ElementName for {guid}: {e}"))?;
            added += 1;
        }
    }
    Ok(added)
}

/// `wsl --shutdown` so vmcompute re-reads freshly-registered GUIDs on the next
/// boot. Run only at install time. Terminates ALL running distros.
pub fn wsl_shutdown() -> Result<(), String> {
    let out = std::process::Command::new("wsl.exe")
        .arg("--shutdown")
        .output()
        .map_err(|e| format!("spawn `wsl --shutdown`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`wsl --shutdown` failed (exit {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}
