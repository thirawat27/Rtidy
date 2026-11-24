# Architecture note — rtidy vs Kondo-GUI

Pass level: **Focus Mode**
Trigger: คำถามเป็นเรื่อง "กลไกการทำงาน/ฟีเจอร์ระดับระบบ" ไม่ใช่ bug เดียว ๆ และกระทบ
ขอบเขตหลายโมดูล (targets / discover / shared / engine / brain / gui)
Risk ถ้าใช้ Scan Mode: ข้อเสนอจะไปชนกับ coupling ที่มีอยู่โดยไม่รู้ตัว
Scope: โครงสร้างปัจจุบันของ rtidy + ช่องว่างเชิงแข่งขันกับ Kondo/kondo-ui
ไม่อยู่ใน scope: การแก้โค้ด (ทุกข้อเสนอคือ *proposed*, ยังไม่ได้ทำ)

## 1. สิ่งที่ตรวจจริง (evidence)

`Cargo.toml`, `src/main.rs`, `src/engine.rs`, `src/targets.rs`, `src/discover.rs`,
`src/config.rs`, `src/util.rs`, `src/shared.rs` (ส่วนหัว API), `src/brain/mod.rs`
(API + docs), `src/gui.rs` (state/tabs/keep marker), `README.md`
ข้อมูล Kondo มาจากการค้นเว็บ (README/release notes ของ tbillington/kondo) — `Inferred`, `Verify first: Yes`

## 2. ข้อเท็จจริงที่ยืนยันแล้ว (`Direct`)

- rtidy ผูกกับ Cargo ทั้งระบบ: `targets.rs:is_cargo_target` (ชื่อ `target` + `Cargo.toml` ข้าง ๆ),
  `discover.rs:is_project` (`Cargo.toml`), `config.rs:holds_cargo_project`,
  `shared.rs` แก้ `~/.cargo/config.toml`, `cache.rs` ล้าง `~/.cargo`,
  `engine.rs:project_obs` ใช้ mtime ของ `Cargo.toml` เป็น last_build สำรอง
- จุดแข็งที่ Kondo ไม่มี: shared target dir (ป้องกันที่ต้นเหตุ), brain (logistic regression
  ทำนาย reuse + disk forecast + anomaly + quiet hour), ตัวตั้งเวลาข้ามแพลตฟอร์ม
  + catch-up, doctor, prune cache, i18n (EN/TH), wizard, CLI 14 คำสั่ง
- จุดแข็งของ Kondo: รองรับ ~20 ecosystem (Cargo, Node, Gradle, Maven, CMake, Composer,
  Elixir, Godot, Jupyter/Pixi, Pub, SBT, Stack, Swift, Unity, Unreal, Zig, .NET …)
  และ UI แยก artifact / non-artifact ต่อโปรเจกต์ (`Inferred`)
- rtidy ลบแบบถาวร: `util.rs:157 remove_known` → `fs::remove_dir_all` ไม่มี trash/undo
- GUI ไม่มีโหมด preview: `engine::fix_all` ส่ง `dry_run = false` คงที่
  (`targets::clean_found(found, 0, false)`, `shared::gc(.., false)`); `dry_run` มีแค่ใน CLI
- GUI รับผลเป็น `Vec<String>` log + `FixReport { freed, warnings }` เท่านั้น (`gui.rs:Msg`)
- `say!` กระจายใน engine(10)/shared(8)/targets(3)/main(3)/cache(1)/util(1) — ชั้น logic พิมพ์เอง
- GUI ตั้ง keep ได้แล้วผ่านไฟล์ marker (`gui.rs:672` เขียน `.rtidy-keep`)

## 3. Findings

### F1 — Cargo coupling กระจายทั่ว ไม่มี boundary เดียว
- Evidence: 6 โมดูลข้างบนตรวจ `Cargo.toml` / `target` / `~/.cargo` เอง
- Impact: การรองรับ ecosystem อื่น (จุดขายหลักของ Kondo) ต้องแก้ 6–7 ไฟล์พร้อมกัน
  และ brain/schedule/doctor จะไม่ได้ประโยชน์อัตโนมัติ
- Severity: **High** · Confidence: `Direct`
- Direction (proposed, เล็กที่สุดที่ปลอดภัย): เพิ่ม `src/eco.rs` เป็น *ตาราง descriptor*
  (ไม่ใช่ trait object): `{ id, manifest: &[&str], artifact_dirs: &[&str], marker_files,
  shared_store: Option<SharedStore>, user_cache: Option<PathBuf> }` แล้วให้
  `targets`/`discover`/`config` อ่านตารางนี้แทนค่าคงที่ของตัวเอง; `shared.rs` คงไว้
  เป็น implementation ของ `SharedStore::Cargo` ตัวแรก

### F2 — engine ปน decide + execute + print เข้าด้วยกัน
- Evidence: `targets::clean_found` ทั้งคัดเหยื่อ ลบ และ `say!`; `engine::fix_all` เรียกเป็นลำดับ
  แล้วคืนแค่ยอดรวม; GUI ได้ข้อความ ไม่ได้โครงสร้าง
- Impact: ปิดทางทำ preview / undo / `--json` / CI exit code ทั้งหมด — ฟีเจอร์ P1–P3 ข้างล่าง
  ติดคอขวดนี้ก่อน
- Severity: **High** · Confidence: `Direct`
- Direction (proposed): แยก `Plan { actions: Vec<Action>, est_freed, est_rebuild_cost }`
  ออกจาก `apply(plan) -> Outcome`; `say!` ย้ายไปอยู่ขอบนอก (main/gui) เท่านั้น

### F3 — ลบถาวรโดยไม่มีตาข่ายรับ
- Evidence: `remove_dir_all` ตรง ๆ, ไม่มี trash/quarantine/undo, GUI ไม่มี preview
- Impact: ความเชื่อใจ — Kondo เองเตือนว่าตัวมัน "เท่ากับ rm -rf" ซึ่งเป็นช่องที่ rtidy
  ชนะได้ด้วยงานไม่มาก; และ bug ใน policy ใด ๆ แปลเป็นข้อมูลหายทันที
- Severity: **High** · Confidence: `Direct`

### F4 — `project_key` คือ path string
- Evidence: `engine.rs:project_key` = `dir.display().to_string()`
- Impact: ย้าย/เปลี่ยนชื่อโฟลเดอร์ = โปรเจกต์ใหม่ในสายตา brain, ประวัติ reuse หาย,
  และ state ไฟล์โตขึ้นเรื่อย ๆ
- Severity: **Medium** · Confidence: `Direct`
- Direction: key แบบคู่ (path + fingerprint จาก package name / git remote) เลือกอันที่หาเจอ

### F5 — flow conflict: ตัว skip ชน ecosystem ใหม่
- Evidence: `targets.rs:skip_dir` ข้าม `node_modules`, `.gradle`, `.nuget`;
  `discover.rs:skip` ข้าม `target`, `node_modules`, `__pycache__`, `.venv`, `vendor`
- Impact: ถ้าเพิ่ม ecosystem โดยไม่แก้สองลิสต์นี้ scan จะ "มองไม่เห็น" ของที่ตั้งใจจะลบ
  และสองลิสต์มีเหตุผลคนละชุด (กันขยะ vs กัน dependency) ซึ่งจะพันกัน
- Severity: **Medium** · Confidence: `Direct`
- Direction: ให้ skip มาจากตาราง F1 (`skip_unless_ecosystem_enabled`) ไม่ใช่ const สองชุด

## 4. ข้อเสนอฟีเจอร์/กลไก (ทุกข้อ = proposed, ต้องอนุมัติ)

เรียงตาม "ชนะต่อแรงที่ลง" — ไม่ใช่ขนาดงาน

| # | ฟีเจอร์ | ทำไมชนะ Kondo-GUI | ติดกับ |
|---|---|---|---|
| P1 | **Plan → Apply + Trash/Undo**: preview ทุกการลบใน GUI, ลงถังขยะ OS หรือ quarantine 24 ชม., `rtidy undo` | Kondo = rm -rf ทางเดียว; นี่คือจุดที่ผู้ใช้กลัวที่สุด | F2, F3 |
| P2 | **Multi-ecosystem ผ่านตาราง descriptor** (Node/Gradle/Python/.NET/Unity/Zig/Go) | ลบจุดขายเดียวที่ Kondo มีเหนือ rtidy | F1, F5 |
| P3 | **Shared store ต่อ ecosystem** (pnpm store, `GRADLE_USER_HOME`, `GOCACHE`, uv cache, NuGet global, ccache) | Kondo ไม่มีแนวคิด "ป้องกัน" เลย ลบแล้วมันกลับมาใหม่ทุกครั้ง — อันนี้คือ moat จริงของ rtidy | P2 |
| P4 | **Rebuild-cost ranking**: จัดอันดับด้วย `size × P(reuse) × เวลา build ที่คาด` แล้วบอกเป็น "คืน 12 GB แลกกับ build 40 วิ" | Kondo เรียงตามขนาดอย่างเดียว; rtidy มี P(reuse) อยู่แล้ว เหลือแค่ประมาณเวลา | brain มีพอแล้ว |
| P5 | **Watch mode (daemon/debounce)**: ตัด incremental หลัง build จบ N นาที แทนรอบวันละครั้ง | ทั้ง Kondo และ rtidy ตอนนี้เป็น batch; real-time คือระดับที่ไม่มีใครอยู่ | F2 |
| P6 | **โหมดทีม/CI**: `.rtidy.toml` ต่อ repo (commit ได้), `--json`, exit code, `rtidy budget 50G` สำหรับ runner | Kondo ไม่มีอะไรสำหรับทีม/CI | F2 |
| P7 | **Drill-down "ทำไมมันใหญ่"** ใน GUI (ต่อ profile/ต่อ crate/ต่อ ecosystem) + รายงานสะสม "ปีนี้ประหยัดไป N GB" | kondo-ui แสดงแค่ artifact vs non-artifact | snapshot ขยายฟิลด์ |

ลำดับที่แนะนำ: **P1 → P2 → P3** (P1 เปิดทาง P2/P3 และได้ความเชื่อใจก่อนขยายขอบเขตการลบ),
แล้ว P4 (เล็ก ผลสูง) → P6 → P5 → P7

## 5. Open questions

1. เป้าหมายตัวตนของ rtidy ยังเป็น "เครื่องมือ Rust" หรือจะเป็น "ตัวจัดการ build artifact ทุกภาษา"?
   คำตอบนี้เปลี่ยนทั้งชื่อ crate, keywords, README และขนาดงาน P2 ทั้งก้อน
2. ยอมรับ dependency เพิ่มไหม — `trash` (P1), `notify` (P5)? ตอนนี้ Cargo.toml ตั้งใจคุม tree ไว้แน่น
3. `.rtidy.toml` ต่อ repo จะมีสิทธิ์เหนือ hard limit ใน Settings หรือไม่
4. MSRV 1.99 และ `--no-default-features` ต้องยังใช้ได้กับทุกข้อเสนอใช่ไหม

## 6. Risks

- **R1** ขยาย ecosystem = ขยายรัศมีการลบ ก่อนมี undo (F3) → ทำ P1 ก่อน P2 คือข้อบังคับ ไม่ใช่ความชอบ
- **R2** brain ถูก calibrate ด้วยพฤติกรรม Cargo; เอา feature เดิมไปใช้กับ node_modules อาจทำนายเพี้ยน — ควรแยก weight ต่อ ecosystem (`Inferred`)
- **R3** P5 (daemon) เพิ่ม process ที่ค้างอยู่ตลอด = ผิวสัมผัสใหม่ทั้ง 3 OS
- **R4** ตาราง descriptor แบบ "รองรับ 20 ภาษา" โดยทดสอบจริงแค่ 2–3 = ความถูกต้องปลอม น่าเสียหายกว่าไม่รองรับ

## 7. Validation gate

1. Claim traceability: ข้อเท็จจริงทุกข้อในหัวข้อ 2 อ้างไฟล์/บรรทัดที่อ่านแล้ว; ข้อมูล Kondo ติดป้าย `Inferred` + `Verify first: Yes`
2. Scope alignment: ตรงกับ intake (ฟีเจอร์/กลไกเพื่อเอาชนะ Kondo-GUI) ไม่ได้ขยายไปแก้โค้ด
3. Handoff readiness: มี open questions, risks และลำดับงานที่ปลอดภัย (P1 ก่อน P2)
