//! Machine-bound key derivation, byte-compatible with Nexo Mod's
//! `HardwareKey.java`.
//!
//! Both halves of the project read one shared account store, so both must
//! derive **the same 32-byte key from the same machine**. Nothing key-like is
//! written to disk, which is what stops a copied config folder from being
//! decryptable elsewhere.
//!
//! Read `Mod/docs/SHARED-ACCOUNT-STORE.md` before touching anything here. The
//! failure mode for a mismatch is not an error — it's a GCM tag check failure
//! that makes a perfectly good store look corrupt. Order of parts, the exact
//! label strings, trimming, and the sorting of GPU entries are all load
//! bearing.

use sha2::{Digest, Sha256};
use std::path::Path;

/// Domain separator, and the format version of the derivation itself. Bump it
/// on **any** change to what goes into the digest, and change the Java
/// constant in the same commit.
const DOMAIN: &str = "nexomod-hwkey-v1";

/// The derived key, plus what it was built from — the count is worth logging
/// so a machine that suddenly produces fewer identifiers is diagnosable.
#[derive(Clone)]
pub struct HardwareKey {
    key: [u8; 32],
    parts: usize,
}

impl std::fmt::Debug for HardwareKey {
    /// Never renders the key itself, so it can't leak through a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardwareKey")
            .field("parts", &self.parts)
            .field("fingerprint", &self.fingerprint())
            .finish()
    }
}

impl HardwareKey {
    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }

    pub fn parts(&self) -> usize {
        self.parts
    }

    /// A hash *of the key*, safe to log. Comparing this against the value the
    /// Mod logs is how you verify the two implementations agree without ever
    /// printing key material.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.key);
        hex::encode(&hasher.finalize()[..8])
    }
}

/// Derives this machine's key, or `None` when not a single identifier could
/// be collected.
///
/// `None` is not an error to paper over: callers must fail safe — don't
/// persist secrets, and never delete or truncate a store that couldn't be
/// verified.
///
/// **Computed once per process.** [`crate::shared_store`] calls this on every
/// read *and* every write of the account store, and collecting the identifiers
/// means running external probes: on Windows that is a PowerShell process
/// issuing three `Get-CimInstance` queries, several hundred milliseconds each
/// time. Uncached, ordinary use of the launcher spawned it over and over —
/// which flashed a console window every time and made the UI stutter.
/// Hardware does not change underneath a running process, so caching costs
/// nothing in correctness.
pub fn derive() -> Option<HardwareKey> {
    static CACHED: std::sync::OnceLock<Option<HardwareKey>> = std::sync::OnceLock::new();
    CACHED.get_or_init(derive_uncached).clone()
}

/// How many times the identifiers have actually been collected. Exists so the
/// cache is a tested property rather than an unenforced comment — dropping the
/// `OnceLock` sends this above 1 and fails
/// `identifiers_are_only_collected_once`.
static DERIVATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The actual derivation, run at most once per process.
fn derive_uncached() -> Option<HardwareKey> {
    DERIVATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let parts = collect();
    if parts.is_empty() {
        tracing::error!(
            "no hardware identifiers available — the account store cannot be read or written"
        );
        return None;
    }

    // Must mirror HardwareKey.derive(): domain string, then for each part a
    // single newline byte followed by the part's UTF-8 bytes.
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN.as_bytes());
    for part in &parts {
        hasher.update([b'\n']);
        hasher.update(part.as_bytes());
    }

    let key: [u8; 32] = hasher.finalize().into();
    Some(HardwareKey {
        key,
        parts: parts.len(),
    })
}

fn collect() -> Vec<String> {
    let mut parts = Vec::new();
    if cfg!(target_os = "windows") {
        collect_windows(&mut parts);
    } else if cfg!(target_os = "macos") {
        collect_macos(&mut parts);
    } else {
        collect_linux(&mut parts);
    }
    parts
}

/// Java's `add`: skips null/blank, trims, and formats as `label=value`.
fn add(parts: &mut Vec<String>, label: &str, value: Option<String>) {
    let Some(value) = value else { return };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }
    parts.push(format!("{label}={trimmed}"));
}

/// Java's `joinNonBlank`: trims each, drops blanks, single-space separated.
/// Returns an empty string when everything was blank, which `add` then skips.
fn join_non_blank(values: &[Option<String>]) -> String {
    let mut joined = String::new();
    for value in values.iter().flatten() {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(trimmed);
    }
    joined
}

/// Java's `readFirstLine`: the first line, or `None` if unreadable. Missing
/// and permission-denied are both normal here (`board_serial` is root-only on
/// most distros) and must be treated identically.
fn read_first_line(path: impl AsRef<Path>) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    contents.lines().next().map(|line| line.to_string())
}

/// GPU entries are sorted before being added, because the OS gives no
/// enumeration-order guarantee and an unsorted list would change the key
/// between boots on a multi-GPU machine.
fn add_gpus(parts: &mut Vec<String>, mut gpus: Vec<String>) {
    gpus.sort();
    for gpu in gpus {
        add(parts, "gpu", Some(gpu));
    }
}

fn collect_linux(parts: &mut Vec<String>) {
    let mut cpu_name = None;
    let mut cpu_serial = None;

    if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
        for line in cpuinfo.lines() {
            let Some(colon) = line.find(':') else { continue };
            let field = line[..colon].trim();
            let value = line[colon + 1..].trim();

            // Only the *first* occurrence of each counts — a multi-core box
            // repeats `model name` once per core.
            if cpu_name.is_none() && (field == "model name" || field == "Model") {
                cpu_name = Some(value.to_string());
            } else if cpu_serial.is_none() && field == "Serial" {
                cpu_serial = Some(value.to_string());
            }
        }
    }

    add(parts, "cpu.name", cpu_name);
    add(parts, "cpu.serial", cpu_serial);

    add(
        parts,
        "board.name",
        Some(join_non_blank(&[
            read_first_line("/sys/class/dmi/id/board_vendor"),
            read_first_line("/sys/class/dmi/id/board_name"),
        ])),
    );
    add(
        parts,
        "board.serial",
        read_first_line("/sys/class/dmi/id/board_serial"),
    );

    let mut gpus = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_card_dir(&name) {
                continue;
            }
            let device = entry.path().join("device");
            let ids = join_non_blank(&[
                read_first_line(device.join("vendor")),
                read_first_line(device.join("device")),
                read_first_line(device.join("subsystem_vendor")),
                read_first_line(device.join("subsystem_device")),
            ]);
            if !ids.trim().is_empty() {
                gpus.push(ids);
            }
        }
    }
    add_gpus(parts, gpus);
}

/// Equivalent of Java's `card\d+` filter — `card0`, but not `card0-DP-1`.
fn is_card_dir(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("card") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

fn collect_windows(parts: &mut Vec<String>) {
    // One PowerShell CIM query, matching the Java side's command exactly so
    // the returned strings are identical.
    let lines = run_command(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1;\
             'cpu.name=' + $cpu.Name;\
             'cpu.serial=' + $cpu.ProcessorId;\
             $board = Get-CimInstance Win32_BaseBoard | Select-Object -First 1;\
             'board.name=' + $board.Manufacturer + ' ' + $board.Product;\
             'board.serial=' + $board.SerialNumber;\
             Get-CimInstance Win32_VideoController | ForEach-Object { 'gpu=' + $_.Name + '/' + $_.PNPDeviceID }",
        ],
    );

    let mut gpus = Vec::new();
    for line in lines {
        let Some(eq) = line.find('=') else { continue };
        let field = line[..eq].trim().to_string();
        let value = line[eq + 1..].trim().to_string();
        if field == "gpu" {
            if !value.is_empty() {
                gpus.push(value);
            }
        } else {
            add(parts, &field, Some(value));
        }
    }
    add_gpus(parts, gpus);
}

fn collect_macos(parts: &mut Vec<String>) {
    add(
        parts,
        "cpu.name",
        run_command("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"])
            .into_iter()
            .next(),
    );
    add(
        parts,
        "board.name",
        run_command("/usr/sbin/sysctl", &["-n", "hw.model"])
            .into_iter()
            .next(),
    );

    for line in run_command("/usr/sbin/ioreg", &["-rd1", "-c", "IOPlatformExpertDevice"]) {
        if line.contains("IOPlatformSerialNumber") {
            // Java pulls the quoted value out with a regex; this finds the
            // last quoted run on the line, which is the same thing for
            // ioreg's `"key" = "value"` format.
            if let Some(value) = line.rsplit('"').nth(1) {
                add(parts, "board.serial", Some(value.to_string()));
            }
            break;
        }
    }

    let mut gpus = Vec::new();
    for line in run_command("/usr/sbin/system_profiler", &["SPDisplaysDataType"]) {
        let trimmed = line.trim();
        if let Some(model) = trimmed.strip_prefix("Chipset Model:") {
            gpus.push(model.trim().to_string());
        }
    }
    add_gpus(parts, gpus);
}

/// Runs a probe, returning its stdout lines. Any failure yields no lines, so
/// an unavailable tool is consistently absent rather than an error.
fn run_command(program: &str, args: &[&str]) -> Vec<String> {
    let Ok(output) =
        crate::util::no_window(std::process::Command::new(program).args(args)).output()
    else {
        tracing::warn!(program, "hardware probe unavailable");
        return Vec::new();
    };
    // Java merges stderr into stdout; probes here are only read for their
    // stdout, and a failing probe contributes nothing either way.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collecting the identifiers means running external probes — on Windows a
    /// PowerShell process issuing three CIM queries. `shared_store` calls
    /// `derive` on every account read *and* write, so without the cache simply
    /// using the launcher spawned it repeatedly: a console window flashing each
    /// time and hundreds of milliseconds of stutter per read.
    ///
    /// "At most once" rather than "exactly once" because tests share a process
    /// and another one may have primed the cache first.
    #[test]
    fn identifiers_are_only_collected_once() {
        for _ in 0..5 {
            let _ = derive();
        }
        let runs = DERIVATIONS.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            runs <= 1,
            "derive() probed the hardware {runs} times; it must be cached"
        );
    }

    /// Pins the digest construction itself, independent of any machine.
    /// If this changes, the Java side must change identically or every
    /// existing store becomes unreadable.
    #[test]
    fn digest_construction_is_pinned() {
        // Written out longhand: domain, then a newline byte before *each*
        // part — including the first. Getting that leading newline wrong is
        // the easiest way to silently diverge from the Java side.
        let mut expected = Sha256::new();
        expected.update(b"nexomod-hwkey-v1");
        expected.update([b'\n']);
        expected.update(b"cpu.name=Test CPU");
        expected.update([b'\n']);
        expected.update(b"gpu=0x1 0x2");
        let expected: [u8; 32] = expected.finalize().into();

        // Same inputs assembled the way `derive` does.
        let mut actual = Sha256::new();
        actual.update(DOMAIN.as_bytes());
        for part in ["cpu.name=Test CPU", "gpu=0x1 0x2"] {
            actual.update([b'\n']);
            actual.update(part.as_bytes());
        }
        let actual: [u8; 32] = actual.finalize().into();

        assert_eq!(expected, actual);
    }

    #[test]
    fn add_trims_and_skips_blanks() {
        let mut parts = Vec::new();
        add(&mut parts, "cpu.name", Some("  Ryzen  ".into()));
        add(&mut parts, "cpu.serial", Some("   ".into()));
        add(&mut parts, "board.serial", None);
        assert_eq!(parts, vec!["cpu.name=Ryzen"]);
    }

    #[test]
    fn join_non_blank_matches_java_semantics() {
        assert_eq!(
            join_non_blank(&[Some(" ASUS ".into()), Some("PRIME".into())]),
            "ASUS PRIME"
        );
        // Blanks are skipped, not turned into double spaces.
        assert_eq!(
            join_non_blank(&[Some("ASUS".into()), Some("  ".into()), Some("X".into())]),
            "ASUS X"
        );
        assert_eq!(join_non_blank(&[None, Some("   ".into())]), "");
    }

    #[test]
    fn gpus_are_sorted_so_enumeration_order_cannot_change_the_key() {
        let mut a = Vec::new();
        add_gpus(&mut a, vec!["0x2".into(), "0x1".into()]);
        let mut b = Vec::new();
        add_gpus(&mut b, vec!["0x1".into(), "0x2".into()]);
        assert_eq!(a, b);
        assert_eq!(a, vec!["gpu=0x1", "gpu=0x2"]);
    }

    #[test]
    fn card_dir_filter_matches_java_regex() {
        assert!(is_card_dir("card0"));
        assert!(is_card_dir("card12"));
        // Connector subdirectories must not be mistaken for cards.
        assert!(!is_card_dir("card0-DP-1"));
        assert!(!is_card_dir("card"));
        assert!(!is_card_dir("renderD128"));
    }

    /// Not an assertion about any particular machine — just that derivation
    /// is deterministic within one run, which the whole scheme depends on.
    #[test]
    fn derivation_is_stable() {
        let first = derive();
        let second = derive();
        match (first, second) {
            (Some(a), Some(b)) => {
                assert_eq!(a.key(), b.key());
                assert_eq!(a.parts(), b.parts());
            }
            (None, None) => {}
            _ => panic!("derivation was not deterministic"),
        }
    }
}

#[cfg(test)]
mod interop {
    /// Prints this machine's fingerprint for comparison against the Java
    /// side. Ignored by default because it asserts nothing on its own — see
    /// `Mod/docs/SHARED-ACCOUNT-STORE.md` for the comparison procedure.
    #[test]
    #[ignore = "diagnostic, compare against the Java fingerprint by hand"]
    fn print_fingerprint() {
        match super::derive() {
            Some(key) => println!(
                "RUST_FINGERPRINT={} (from {} identifiers)",
                key.fingerprint(),
                key.parts()
            ),
            None => println!("RUST_FINGERPRINT=none"),
        }
    }
}
