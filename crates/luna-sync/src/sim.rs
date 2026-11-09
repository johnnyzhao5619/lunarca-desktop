//! 确定性模拟器（ADR-148 ⑬；方案 §10，照 Dropbox CanopyCheck 的做法）：几台虚拟设备对着同一个
//! 内存服务端，按随机种子交替做编辑、新建、删除、改名、「说不准」的消失与复原、断网、服务端
//! 换纪元、同步；每个种子都要满足五条不变量：
//!
//! 1. **收敛**：静默后所有设备与服务端是同一份内容（按需设备上仅云端的录音按云端内容算，且
//!    确实不在本机）；
//! 2. **不丢**：一轮同步改掉或拿掉的任何本地内容，都能从「服务端历史 ∪ 各设备文件 ∪ 回收站 ∪
//!    留下的版本」里找回；
//! 3. **没观察到的消失不产生删除**：一台设备上「说不准」的文件、仅云端（按需录音，ADR-156）的
//!    文件，不会因它的同步变成墓碑；
//! 4. **没有冲突标记**：任何文件里都不出现 `<<<<<<<`；
//! 5. **没有撞名**：同一台设备上没有两个文件的折叠路径键相同。
//!
//! 种子数默认跑一小批；CI 门禁另跑一万个（[`ten_thousand_seeds`]，release）；`LUNA_SYNC_SIM_SEEDS=<n>`
//! 跑更多（夜间长跑），`LUNA_SYNC_SIM_SEED=<s>` 只跑一个（复现失败用，失败信息里带着种子）；
//! `LUNA_SYNC_SIM_TRACE=1` 打出静默阶段每一轮的报告与不收敛时的基线 / 服务端头。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use uuid::Uuid;

use crate::engine::{Engine, EngineConfig, SyncAbort};
use crate::policy::RecordingsMode;
use crate::ports::RemoteError;
use crate::state::{path_key, Binding, SyncState};
use crate::testing::{FakeServer, MemVault};
use luna_sync_core::crypto::{Cmk, KeyRing};
use luna_sync_core::manifest::{assemble, open_entry};

const LOCAL_ID: &str = "0190f0f0-0000-7000-8000-00000000c0de";
const POOL: &[&str] = &[
    "a.md",
    "b.md",
    "notes/c.md",
    "notes/d.md",
    "img/e.png",
    "f.md",
    "rec/g.wav",
];

/// 二进制文件（图片与录音）：内容随机、改就整份换。
fn is_binary(path: &str) -> bool {
    path.ends_with(".png") || crate::policy::is_media(path)
}

fn extension(path: &str) -> &str {
    path.rsplit_once('.').map_or("", |(_, ext)| ext)
}

/// xorshift64*：种子决定一切随机选择。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        (!items.is_empty()).then(|| &items[self.below(items.len())])
    }
}

struct Device {
    vault: MemVault,
    state: SyncState,
    /// 「说不准」期间拿走的文件（复原时原样放回）。
    stashed: BTreeMap<String, Vec<u8>>,
    /// 录音策略：0 号设备恒为全部下载（收敛的参照），其余随机。
    mode: RecordingsMode,
}

struct Sim {
    seed: u64,
    rng: Rng,
    server: Arc<FakeServer>,
    ring: KeyRing,
    devices: Vec<Device>,
    /// 服务端提交过的内容（解密缓存）。
    committed: HashMap<(Uuid, i64), Vec<u8>>,
    step: usize,
}

impl Sim {
    fn new(seed: u64, devices: usize) -> Self {
        let vault_id = Uuid::from_u128(u128::from(seed) + 1);
        let server = Arc::new(FakeServer::new(vault_id));
        let ring = KeyRing::new(vault_id, &[Cmk::from_bytes(1, [7; 32])]).unwrap();
        let mut rng = Rng::new(seed ^ 0x005e_ed0f_ca11);
        let devices = (0..devices)
            .map(|i| {
                let mode = if i > 0 && rng.chance(50) {
                    RecordingsMode::OnDemand
                } else {
                    RecordingsMode::Full
                };
                let mut state = SyncState::in_memory().unwrap();
                state
                    .bind(&Binding {
                        remote_vault_id: vault_id,
                        server_origin: "https://sim.test".into(),
                        local_vault_id: LOCAL_ID.into(),
                        created_ms: 0,
                        owner: crate::state::BindingOwner {
                            server_bucket: "sim.test".into(),
                            account_id: "acc".into(),
                            account_label: "a***@sim.test".into(),
                        },
                    })
                    .unwrap();
                Device {
                    vault: MemVault::new(LOCAL_ID),
                    state,
                    stashed: BTreeMap::new(),
                    mode,
                }
            })
            .collect();
        Self {
            seed,
            rng: Rng::new(seed),
            server,
            ring,
            devices,
            committed: HashMap::new(),
            step: 0,
        }
    }

    fn fail(&self, what: &str) -> ! {
        panic!("seed {} step {}: {what}", self.seed, self.step);
    }

    fn content(&mut self, path: &str, device: usize) -> Vec<u8> {
        if is_binary(path) {
            let len = 4 + self.rng.below(12);
            return (0..len).map(|_| self.rng.next() as u8).collect();
        }
        let lines = 3 + self.rng.below(4);
        (0..lines)
            .map(|i| format!("line {i} by device {device} at step {}\n", self.step))
            .collect::<String>()
            .into_bytes()
    }

    /// 改一行（让两台设备的改动常常落在不同的行上，合并才有机会）。
    fn edit(&mut self, device: usize, path: &str) {
        let vault = &self.devices[device].vault;
        let Some(old) = vault.get(path) else { return };
        if is_binary(path) {
            let fresh = self.content(path, device);
            self.devices[device].vault.put(path, &fresh);
            return;
        }
        let text = String::from_utf8_lossy(&old).into_owned();
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let at = self.rng.below(lines.len());
        lines[at] = format!("edited {at} by device {device} at step {}", self.step);
        let new = lines.join("\n") + "\n";
        self.devices[device].vault.put(path, new.as_bytes());
    }

    fn act(&mut self, device: usize) {
        let files: Vec<String> = self.devices[device].vault.snapshot().into_keys().collect();
        match self.rng.below(100) {
            0..=29 => {
                if let Some(path) = self.rng.pick(&files).cloned() {
                    self.edit(device, &path);
                }
            }
            30..=44 => {
                let path = (*self.rng.pick(POOL).unwrap()).to_owned();
                if !self.devices[device].vault.snapshot().contains_key(&path) {
                    let body = self.content(&path, device);
                    self.devices[device].vault.put(&path, &body);
                }
            }
            45..=54 => {
                if let Some(path) = self.rng.pick(&files).cloned() {
                    self.devices[device].vault.remove(&path);
                }
            }
            55..=62 => {
                let to = (*self.rng.pick(POOL).unwrap()).to_owned();
                if let Some(from) = self.rng.pick(&files).cloned() {
                    let same_kind = extension(&from) == extension(&to);
                    if same_kind && !files.contains(&to) {
                        self.devices[device].vault.rename(&from, &to);
                    }
                }
            }
            63..=67 => {
                if let Some(path) = self.rng.pick(&files).cloned() {
                    let bytes = self.devices[device].vault.get(&path).unwrap();
                    self.devices[device].vault.make_uncertain(&path);
                    self.devices[device].stashed.insert(path, bytes);
                }
            }
            68..=72 => self.restore(device),
            73 => self.server.fail_next(RemoteError::Network("sim".into())),
            74 => self.server.lose_next_commit_reply(),
            75 => self.server.change_epoch(),
            // 按需设备：打开一个仅云端的录音（按需下载），或释放一个录音的本机空间。
            76..=79 if self.devices[device].mode == RecordingsMode::OnDemand => {
                self.on_demand(device);
            }
            _ => {
                let allow = self.rng.chance(50);
                self.sync(device, allow);
            }
        }
    }

    fn engine<'a>(&'a self, device: usize, allow_bulk_delete: bool) -> Engine<'a> {
        Engine {
            remote: self.server.as_ref(),
            local: &self.devices[device].vault,
            ring: &self.ring,
            config: EngineConfig {
                conflict_label: "冲突副本".into(),
                today: "2026-10-09".into(),
                allow_bulk_delete,
                now_ms: self.step as i64,
                max_chunks_per_entry: crate::engine::MAX_CHUNKS_PER_ENTRY,
                max_file_bytes: None,
                recordings: self.devices[device].mode,
                defer_large: false,
            },
        }
    }

    /// 按需设备上的「打开录音」与「释放本机空间」。
    fn on_demand(&mut self, device: usize) {
        let bases = self.devices[device].state.bases().unwrap();
        let media: Vec<_> = bases
            .into_iter()
            .filter(|b| !b.deleted && crate::policy::is_media(&b.path))
            .collect();
        let Some(base) = self.rng.pick(&media).cloned() else {
            return;
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut state = std::mem::replace(
            &mut self.devices[device].state,
            SyncState::in_memory().unwrap(),
        );
        let result = {
            let engine = self.engine(device, false);
            if base.cloud_only {
                runtime.block_on(engine.materialize(&mut state, &base.path, &mut |_, _| {}))
            } else {
                engine.evict(&mut state, &base.path)
            }
        };
        self.devices[device].state = state;
        match result {
            Ok(_) | Err(SyncAbort::Remote(_)) => {}
            Err(other) => self.fail(&format!("on-demand action failed: {other}")),
        }
    }

    /// 把「说不准」的文件原样放回（iCloud 下载完成）。
    fn restore(&mut self, device: usize) {
        let stashed = std::mem::take(&mut self.devices[device].stashed);
        for (path, bytes) in stashed {
            // 那个位置这期间被别的东西占了：放不回去，就当用户换了内容（不算同步丢的）。
            if self.devices[device].vault.get(&path).is_none() {
                self.devices[device].vault.put(&path, &bytes);
            }
            self.devices[device].vault.settle(&path);
        }
    }

    fn sync_once(
        &mut self,
        device: usize,
        allow_bulk_delete: bool,
    ) -> Option<crate::engine::RoundReport> {
        let mut state = std::mem::replace(
            &mut self.devices[device].state,
            SyncState::in_memory().unwrap(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let outcome = {
            let engine = self.engine(device, allow_bulk_delete);
            runtime.block_on(engine.run_round(&mut state))
        };
        self.devices[device].state = state;
        match outcome {
            Ok(report) => Some(report),
            Err(
                SyncAbort::Remote(_) | SyncAbort::LocalLooksEmpty | SyncAbort::RemoteLooksEmpty,
            ) => None,
            Err(other) => self.fail(&format!("round aborted unexpectedly: {other}")),
        }
    }

    fn sync(
        &mut self,
        device: usize,
        allow_bulk_delete: bool,
    ) -> Option<crate::engine::RoundReport> {
        let before = self.devices[device].vault.snapshot();
        // 「说不准」按（路径，内容）认：同一路径上先被显式删掉、又新建再变成说不准的，是两个文件。
        let uncertain: HashSet<(String, String)> = self.devices[device]
            .stashed
            .iter()
            .map(|(path, bytes)| (path.clone(), luna_sync_core::manifest::content_hash(bytes)))
            .collect();
        let all_before = self.devices[device].state.bases().unwrap();
        let cloud_only: HashSet<Uuid> = all_before
            .iter()
            .filter(|b| !b.deleted && b.cloud_only)
            .map(|b| b.file_id)
            .collect();
        let bases_before: HashMap<Uuid, (String, String)> = all_before
            .into_iter()
            .filter(|b| !b.deleted)
            .map(|b| (b.file_id, (b.path, b.content_hash)))
            .collect();
        let entries_before = self.server.entries().len();

        let report = self.sync_once(device, allow_bulk_delete);

        // 不变量 3：这一轮新产生的墓碑，不能落在这台设备「说不准」的文件上。
        for entry in &self.server.entries()[entries_before..] {
            if !entry.deleted {
                continue;
            }
            if let Some(key) = bases_before.get(&entry.file_id) {
                if uncertain.contains(key) {
                    self.fail(&format!("an uncertain file became a tombstone: {}", key.0));
                }
                if cloud_only.contains(&entry.file_id) {
                    self.fail(&format!("a cloud-only file became a tombstone: {}", key.0));
                }
            }
        }
        // 不变量 2：这一轮改掉或拿掉的本地内容都找得回。
        let after = self.devices[device].vault.snapshot();
        let mut lost = Vec::new();
        for (path, bytes) in &before {
            if after.get(path) != Some(bytes) {
                lost.push((path.clone(), bytes.clone()));
            }
        }
        if !lost.is_empty() {
            let recoverable = self.recoverable();
            for (path, bytes) in lost {
                if !recoverable.contains(&bytes) {
                    self.fail(&format!("content at {path} was lost by a sync round"));
                }
            }
        }
        self.check_device(device);
        report
    }

    /// 服务端历史里提交过的全部内容。
    fn committed_contents(&mut self) -> Vec<Vec<u8>> {
        for entry in self.server.entries() {
            if entry.deleted || self.committed.contains_key(&(entry.file_id, entry.rev)) {
                continue;
            }
            let Some(ct) = entry.entry_ct.as_deref() else {
                continue;
            };
            match self.decrypt(entry.file_id, entry.rev, ct) {
                Ok((_, bytes)) => {
                    self.committed.insert((entry.file_id, entry.rev), bytes);
                }
                Err(e) => self.fail(&format!("committed entry does not assemble: {e}")),
            }
        }
        self.committed.values().cloned().collect()
    }

    fn decrypt(&self, file_id: Uuid, rev: i64, ct: &[u8]) -> Result<(String, Vec<u8>), String> {
        let plain = open_entry(&self.ring, file_id, rev, ct).map_err(|e| e.to_string())?;
        let server = &self.server;
        let bytes = assemble(&self.ring, &plain, |id| {
            server.blob(id).ok_or_else(|| "missing".into())
        })
        .map_err(|e| e.to_string())?;
        Ok((plain.path, bytes))
    }

    fn recoverable(&mut self) -> HashSet<Vec<u8>> {
        let mut out: HashSet<Vec<u8>> = self.committed_contents().into_iter().collect();
        for device in &self.devices {
            out.extend(device.vault.snapshot().into_values());
            out.extend(device.vault.trash().into_iter().map(|(_, b)| b));
            out.extend(device.vault.versions().into_iter().map(|(_, b)| b));
            out.extend(device.stashed.values().cloned());
        }
        out
    }

    /// 不变量 4、5。
    fn check_device(&self, device: usize) {
        let files = self.devices[device].vault.snapshot();
        let mut keys = HashSet::new();
        for (path, bytes) in &files {
            if !keys.insert(path_key(path)) {
                self.fail(&format!(
                    "two files fold to the same key on device {device}: {path}"
                ));
            }
            if String::from_utf8_lossy(bytes).contains("<<<<<<<") {
                self.fail(&format!("conflict markers in {path}"));
            }
        }
    }

    /// 静默：复原全部「说不准」，然后轮流同步直到一整圈谁都没事可做。
    fn quiesce(&mut self) {
        self.server.heal();
        for device in 0..self.devices.len() {
            self.restore(device);
        }
        for _ in 0..12 {
            let mut busy = false;
            for device in 0..self.devices.len() {
                let report = self
                    .sync(device, true)
                    .unwrap_or_else(|| self.fail("a quiet round failed"));
                if std::env::var_os("LUNA_SYNC_SIM_TRACE").is_some() {
                    eprintln!(
                        "quiet device {device}: {report:?} files={:?}",
                        self.devices[device]
                            .vault
                            .snapshot()
                            .keys()
                            .collect::<Vec<_>>()
                    );
                }
                busy |= report.pushed
                    + report.applied
                    + report.deleted_local
                    + report.deleted_remote
                    + report.conflict_copies
                    + report.deferred
                    > 0;
            }
            if !busy {
                return;
            }
        }
        if std::env::var_os("LUNA_SYNC_SIM_TRACE").is_some() {
            let mut heads: BTreeMap<Uuid, (i64, bool)> = BTreeMap::new();
            for e in self.server.entries() {
                heads.insert(e.file_id, (e.rev, e.deleted));
            }
            for (i, device) in self.devices.iter().enumerate() {
                eprintln!("device {i} cursor {:?}", device.state.cursor().unwrap());
                for base in device.state.bases().unwrap() {
                    eprintln!(
                        "  base {} {:?} rev {} deleted {} | server {:?} | local {}",
                        base.file_id,
                        base.path,
                        base.rev,
                        base.deleted,
                        heads.get(&base.file_id),
                        device.vault.get(&base.path).is_some()
                    );
                }
            }
            eprintln!("server head {}", self.server.head_seq());
        }
        self.fail("did not converge within 12 quiet cycles");
    }

    /// 一台设备眼里的库：本机文件，加上仅云端的文件按云端那一版的内容（它们必须确实不在本机）。
    fn device_view(&mut self, device: usize) -> BTreeMap<String, Vec<u8>> {
        let mut view = self.devices[device].vault.snapshot();
        let mut latest: HashMap<Uuid, crate::ports::RemoteEntry> = HashMap::new();
        for entry in self.server.entries() {
            latest.insert(entry.file_id, entry);
        }
        for base in self.devices[device].state.bases().unwrap() {
            if base.deleted || !base.cloud_only {
                continue;
            }
            if view.contains_key(&base.path) {
                self.fail(&format!(
                    "device {device}: cloud-only {} is also on disk",
                    base.path
                ));
            }
            let Some(entry) = latest.get(&base.file_id).filter(|e| e.rev == base.rev) else {
                self.fail(&format!(
                    "device {device}: cloud-only {} is not the server's head",
                    base.path
                ));
            };
            let ct = entry.entry_ct.clone().unwrap_or_default();
            match self.decrypt(base.file_id, base.rev, &ct) {
                Ok((path, bytes)) => {
                    view.insert(path, bytes);
                }
                Err(e) => self.fail(&format!("a cloud-only entry does not decrypt: {e}")),
            }
        }
        view
    }

    /// 不变量 1：所有设备与服务端是同一份内容。
    fn check_converged(&mut self) {
        let first = self.device_view(0);
        for i in 1..self.devices.len() {
            if self.device_view(i) != first {
                self.fail(&format!("device {i} differs from device 0 after quiescing"));
            }
        }
        let mut server_view: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut latest: BTreeMap<Uuid, crate::ports::RemoteEntry> = BTreeMap::new();
        for entry in self.server.entries() {
            latest.insert(entry.file_id, entry);
        }
        for entry in latest.into_values().filter(|e| !e.deleted) {
            match self.decrypt(
                entry.file_id,
                entry.rev,
                entry.entry_ct.as_deref().unwrap_or_default(),
            ) {
                Ok((path, bytes)) => {
                    server_view.insert(path, bytes);
                }
                Err(e) => self.fail(&format!("a live entry does not decrypt: {e}")),
            }
        }
        if server_view != first {
            self.fail("the server's live files differ from the devices'");
        }
    }

    fn run(&mut self, steps: usize) {
        for step in 0..steps {
            self.step = step;
            let device = self.rng.below(self.devices.len());
            self.act(device);
        }
        self.step = steps;
        self.quiesce();
        self.check_converged();
    }
}

fn seeds() -> Vec<u64> {
    if let Some(seed) = std::env::var("LUNA_SYNC_SIM_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        return vec![seed];
    }
    let count = std::env::var("LUNA_SYNC_SIM_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300_u64);
    (0..count).collect()
}

fn run_seeds(seeds: impl IntoIterator<Item = u64>) {
    for seed in seeds {
        let devices = 2 + (seed % 2) as usize;
        Sim::new(seed, devices).run(60);
    }
}

/// 随 `cargo test --workspace` 跑的那一小批（调试构建约 10 秒）。
#[test]
fn every_seed_keeps_the_five_invariants() {
    run_seeds(seeds());
}

/// CI 门禁：一万个种子（`cargo test -p luna-sync --release -- --ignored ten_thousand_seeds`，
/// release 约半分钟）。默认忽略，免得调试构建的 `cargo test --workspace` 被它拖上几分钟。
#[test]
#[ignore = "CI 门禁单独跑（release）"]
fn ten_thousand_seeds() {
    run_seeds(10_000..20_000);
}
