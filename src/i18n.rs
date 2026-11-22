//! Two languages: English (source strings) and Thai.
//! `t("English")` returns the text in the current language; `tf` fills `{}` placeholders.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    #[default]
    En,
    Th,
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set(l: Lang) {
    LANG.store(l as u8, Ordering::Relaxed);
}

pub fn get() -> Lang {
    if LANG.load(Ordering::Relaxed) == 1 { Lang::Th } else { Lang::En }
}

/// Thai if the OS language is Thai, otherwise English.
pub fn detect() -> Lang {
    match sys_locale::get_locale() {
        Some(l) if l.to_ascii_lowercase().starts_with("th") => Lang::Th,
        _ => Lang::En,
    }
}

/// Built once, on the first Thai lookup; English never pays for it.
static THAI: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| TH.iter().copied().collect());

pub fn t(en: &'static str) -> &'static str {
    match get() {
        Lang::En => en,
        Lang::Th => THAI.get(en).copied().unwrap_or(en),
    }
}

/// Translate, then replace each `{}` with the next argument.
pub fn tf(en: &'static str, args: &[&str]) -> String {
    let translated = t(en);
    // One allocation: the result is the text plus the arguments, near enough.
    let mut out = String::with_capacity(translated.len() + args.iter().map(|a| a.len()).sum::<usize>());
    let mut parts = translated.split("{}");
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    for (i, p) in parts.enumerate() {
        out.push_str(args.get(i).copied().unwrap_or(""));
        out.push_str(p);
    }
    out
}

const TH: &[(&str, &str)] = &[
    // wizard steps
    ("Welcome", "ยินดีต้อนรับ"),
    ("Folders", "โฟลเดอร์"),
    ("Preferences", "ตั้งค่า"),
    ("Review", "ตรวจสอบ"),
    ("Done", "เสร็จสิ้น"),
    // general
    (" · time picked by AI", " · AI เลือกเวลาให้"),
    ("+  Add folder…", "+  เพิ่มโฟลเดอร์…"),
    ("+  Choose folder…", "+  เลือกโฟลเดอร์…"),
    ("A forecast appears after a few days of history.", "จะเริ่มพยากรณ์ได้เมื่อมีข้อมูลสะสมสักสองสามวัน"),
    ("A tiny on-device model learns what you reuse and when you work, then cleans quietly every day.", "โมเดลขนาดเล็กในเครื่องจะเรียนรู้ว่าคุณใช้อะไรซ้ำและทำงานช่วงไหน แล้วล้างให้เงียบๆ ทุกวัน"),
    ("AI: chance you'll build this project again in the next 14 days", "AI: โอกาสที่คุณจะ build โปรเจกต์นี้อีกภายใน 14 วัน"),
    ("Activity", "กิจกรรม"),
    ("Add at least one folder to continue.", "เพิ่มอย่างน้อยหนึ่งโฟลเดอร์เพื่อไปต่อ"),
    ("Add to Start menu", "เพิ่มใน Start menu"),
    ("Add to app menu", "เพิ่มในเมนูแอป"),
    ("Add", "เพิ่ม"),
    ("All tidy", "เรียบร้อยแล้ว"),
    ("All tidy! Freed {}", "เรียบร้อย! คืนพื้นที่ได้ {}"),
    ("Almost there", "เกือบเสร็จแล้ว"),
    ("Automatic daily clean-up is off", "ยังไม่ได้เปิดการล้างอัตโนมัติรายวัน"),
    ("Automatic daily clean-up", "ล้างอัตโนมัติทุกวัน"),
    ("Back", "ย้อนกลับ"),
    ("Build dependencies without debug info", "build dependency โดยไม่เก็บ debug info"),
    ("Cargo cache", "แคชของ Cargo"),
    ("Cargo downloads older than (0 = keep)", "ไฟล์ดาวน์โหลดของ Cargo เก่ากว่า (0 = เก็บไว้)"),
    ("Clean up automatically every day", "ล้างอัตโนมัติทุกวัน"),
    ("Clean up every day at the time you build least", "ล้างทุกวันในช่วงที่คุณ build น้อยที่สุด"),
    ("Clean up every day at {}", "ล้างทุกวันเวลา {}"),
    ("Clean-up rules", "กฎการล้าง"),
    ("Couldn't add to menu: {}", "เพิ่มลงเมนูไม่สำเร็จ: {}"),
    ("Couldn't change schedule: {}", "เปลี่ยนตารางเวลาไม่สำเร็จ: {}"),
    ("Couldn't save: {}", "บันทึกไม่สำเร็จ: {}"),
    ("Create one shared target folder for all projects", "สร้างโฟลเดอร์ target กลางให้ทุกโปรเจกต์ใช้ร่วมกัน"),
    ("Critical", "วิกฤต"),
    ("Daily clean-up already runs then, so it never slows your builds.", "ตั้งเวลาล้างไว้ช่วงนั้นแล้ว จึงไม่รบกวนตอนคุณ build"),
    ("Daily clean-up off", "ปิดการล้างรายวันแล้ว"),
    ("Daily clean-up on ({})", "เปิดการล้างรายวันแล้ว ({})"),
    ("Daily clean-up time", "เวลาล้างประจำวัน"),
    ("Delete leftover target/ folders (they rebuild into the shared folder)", "ลบโฟลเดอร์ target/ ที่ค้างอยู่ (build ครั้งหน้าจะไปอยู่ในโฟลเดอร์กลาง)"),
    ("Delete project target/ not built for", "ลบ target/ ของโปรเจกต์ที่ไม่ได้ build มา"),
    ("Delete", "ลบ"),
    ("Deleted {} target ({})", "ลบ target ของ {} แล้ว ({})"),
    ("Deleting files can take a minute.", "การลบไฟล์อาจใช้เวลาสักครู่"),
    ("Dependencies are built with full debug info", "dependency ถูก build พร้อม debug info เต็ม"),
    ("Dependencies skip debug info, which is often half the size. Your own code keeps it.", "dependency ไม่เก็บ debug info ซึ่งมักกินพื้นที่ครึ่งหนึ่ง ส่วนโค้ดของคุณยังเก็บไว้เหมือนเดิม"),
    ("Dependencies without debug info (your own code keeps it)", "dependency ไม่เก็บ debug info (โค้ดของคุณยังเก็บไว้)"),
    ("Discard", "ยกเลิก"),
    ("Disk full in about {} days", "ดิสก์จะเต็มในอีกประมาณ {} วัน"),
    ("Disk pressure", "ความตึงของดิสก์"),
    ("Disk space is stable", "พื้นที่ดิสก์คงที่"),
    ("Each project keeps its own target/ folder", "แต่ละโปรเจกต์มีโฟลเดอร์ target/ ของตัวเอง"),
    ("Every day at {}{} · hasn't run yet", "ทุกวันเวลา {}{} · ยังไม่เคยรัน"),
    ("Every day at {}{} · last: {}", "ทุกวันเวลา {}{} · ล่าสุด: {}"),
    ("Every project builds into the same place — no more target/ in each repo, and shared dependencies compile once.", "ทุกโปรเจกต์ build ลงที่เดียวกัน ไม่มี target/ กระจายในแต่ละ repo อีก และ dependency ที่ใช้ร่วมกัน compile แค่ครั้งเดียว"),
    ("Free disk", "พื้นที่ว่าง"),
    ("Free space is growing by about {}/day.", "พื้นที่ว่างเพิ่มขึ้นประมาณ {}/วัน"),
    ("Free space is shrinking by about {}/day. rtidy cleans harder as it gets closer.", "พื้นที่ว่างลดลงประมาณ {}/วัน ยิ่งใกล้เต็ม rtidy จะล้างเข้มขึ้น"),
    ("Freed {}, but: {}", "คืนพื้นที่ได้ {} แต่: {}"),
    ("Get started", "เริ่มเลย"),
    ("Getting tight", "เริ่มตึง"),
    ("Here's what we found and what will happen:", "สิ่งที่เราพบและสิ่งที่จะทำ:"),
    ("How it works: a small model learns which build files you reuse and how fast your disk fills. It runs on this computer in milliseconds; nothing is uploaded.", "ทำงานอย่างไร: โมเดลขนาดเล็กเรียนรู้ว่าไฟล์ build ไหนที่คุณใช้ซ้ำ และดิสก์เต็มเร็วแค่ไหน ประมวลผลในเครื่องนี้ภายในไม่กี่มิลลิวินาที ไม่มีการส่งข้อมูลออกไปไหน"),
    ("How tidy should it be?", "อยากให้เรียบร้อยแค่ไหน?"),
    ("Just save settings", "บันทึกการตั้งค่าอย่างเดียว"),
    ("Keep your shared target folder", "ใช้โฟลเดอร์ target กลางที่มีอยู่ต่อ"),
    ("Keep", "เก็บไว้"),
    ("Keeps your Rust target folders tidy", "จัดการโฟลเดอร์ target ของ Rust ให้เรียบร้อย"),
    ("Language of the app", "ภาษาที่ใช้ในแอป"),
    ("Language", "ภาษา"),
    ("Lean builds", "Lean builds (build แบบประหยัด)"),
    ("Learning your disk usage", "กำลังเรียนรู้การใช้ดิสก์ของคุณ"),
    ("Learns how you work and decides what to clean", "เรียนรู้วิธีทำงานของคุณ แล้วตัดสินใจว่าควรล้างอะไร"),
    ("Learns what you reuse; cleans harder only when the disk is filling up", "เรียนรู้ว่าคุณใช้อะไรซ้ำ และล้างเข้มขึ้นเฉพาะตอนดิสก์ใกล้เต็ม"),
    ("Leftover targets", "target ที่ค้างอยู่"),
    ("Let AI pick the time", "ให้ AI เลือกเวลา"),
    ("Let's stop Rust builds from eating your disk.", "มาหยุดไม่ให้การ build ของ Rust กินพื้นที่ดิสก์กันเถอะ"),
    ("Limits that always apply, with or without AI", "ขีดจำกัดที่ใช้เสมอ ไม่ว่าจะเปิด AI หรือไม่"),
    ("Model is still learning your habits", "โมเดลยังเรียนรู้พฤติกรรมของคุณอยู่"),
    ("Never clean this project", "ไม่ล้างโปรเจกต์นี้"),
    ("Next", "ถัดไป"),
    ("No per-project target/ folders found.", "ไม่พบโฟลเดอร์ target/ แยกตามโปรเจกต์"),
    ("Normally about {}/day. A new toolchain, a big dependency or a release build?", "ปกติประมาณ {}/วัน อาจเป็น toolchain ใหม่, dependency ใหญ่ หรือ release build?"),
    ("Nothing yet this session. Scheduled runs are logged to:", "ยังไม่มีกิจกรรมในรอบนี้ ผลการรันตามเวลาจะบันทึกไว้ที่:"),
    ("Off — turn on to keep things tidy without thinking about it", "ปิดอยู่ เปิดไว้แล้วไม่ต้องคอยดูแลเอง"),
    ("Off", "ปิด"),
    ("One shared target folder", "โฟลเดอร์ target กลางที่เดียว"),
    ("Open dashboard", "เปิดแดชบอร์ด"),
    ("Outcomes learned", "ผลลัพธ์ที่เรียนรู้แล้ว"),
    ("Prediction accuracy {}%", "ความแม่นยำในการทำนาย {}%"),
    ("Prediction accuracy", "ความแม่นยำ"),
    ("Project folders", "โฟลเดอร์โปรเจกต์"),
    ("Projects", "โปรเจกต์"),
    ("Ready to tidy up", "พร้อมจัดการแล้ว"),
    ("Relaxed", "สบาย"),
    ("Remove cargo downloads older than {} days", "ลบไฟล์ดาวน์โหลดของ Cargo ที่เก่ากว่า {} วัน"),
    ("Remove what you're unlikely to reuse first; clean harder when the disk is filling up", "ลบสิ่งที่ไม่น่าจะได้ใช้อีกก่อน และล้างเข้มขึ้นเมื่อดิสก์ใกล้เต็ม"),
    ("Remove", "เอาออก"),
    ("Added 1 project folder", "เพิ่มโฟลเดอร์โปรเจกต์ 1 โฟลเดอร์"),
    ("Added {} project folders", "เพิ่มโฟลเดอร์โปรเจกต์ {} โฟลเดอร์"),
    ("Look through every drive for folders that contain Cargo projects", "ค้นหาทุกไดรฟ์ เพื่อหาโฟลเดอร์ที่มีโปรเจกต์ Cargo อยู่จริง"),
    ("No new project folders found", "ไม่พบโฟลเดอร์โปรเจกต์ใหม่"),
    ("Search this computer", "ค้นหาในเครื่องนี้"),
    ("Searching this computer…", "กำลังค้นหาในเครื่องนี้…"),
    ("Run setup wizard again", "เปิดตัวช่วยตั้งค่าอีกครั้ง"),
    ("Run the daily clean-up at the hour you build least", "ล้างประจำวันในชั่วโมงที่คุณ build น้อยที่สุด"),
    ("Runs quietly in the background via {}", "ทำงานเงียบๆ เบื้องหลังผ่าน {}"),
    ("Save", "บันทึก"),
    ("Scanning your projects…", "กำลังสแกนโปรเจกต์…"),
    ("Settings saved", "บันทึกการตั้งค่าแล้ว"),
    ("Settings", "ตั้งค่า"),
    ("Share one target folder across projects, build dependencies lean, delete leftover target/ folders, trim caches and turn on daily clean-up.", "ใช้โฟลเดอร์ target กลางร่วมกัน, build dependency แบบประหยัด, ลบ target/ ที่ค้าง, ลดแคช และเปิดการล้างรายวัน"),
    ("Shared folder size cap (0 = none)", "ขนาดสูงสุดของโฟลเดอร์กลาง (0 = ไม่จำกัด)"),
    ("Shared folder: drop artifacts unused for", "โฟลเดอร์กลาง: ลบไฟล์ build ที่ไม่ได้ใช้มา"),
    ("Shared target folder, lean builds and daily clean-up are all on.", "เปิดใช้โฟลเดอร์ target กลาง, lean builds และการล้างรายวันครบแล้ว"),
    ("Shared target", "target กลาง"),
    ("Shrinking slowly ({}/day), nothing to worry about.", "ลดลงช้าๆ ({}/วัน) ไม่น่าห่วง"),
    ("Skip setup", "ข้ามการตั้งค่า"),
    ("Smart clean-up", "ล้างอย่างชาญฉลาด"),
    ("Smart mode (AI)", "โหมดอัจฉริยะ (AI)"),
    ("Smart mode is off. Turn it on in Settings to get AI insights.", "โหมดอัจฉริยะปิดอยู่ เปิดได้ในหน้าตั้งค่าเพื่อดูข้อมูลเชิงลึกจาก AI"),
    ("Smart schedule moves the daily clean-up there.", "ตารางเวลาอัจฉริยะจะย้ายเวลาล้างไปช่วงนั้น"),
    ("Smart schedule", "ตารางเวลาอัจฉริยะ"),
    ("Smart, and it cleans itself", "ฉลาด และล้างเองอัตโนมัติ"),
    ("So you can open rtidy like any other app", "เปิด rtidy ได้เหมือนแอปทั่วไป"),
    ("Something went wrong: {}", "เกิดข้อผิดพลาด: {}"),
    ("Start learning your build habits (on this computer only)", "เริ่มเรียนรู้พฤติกรรมการ build ของคุณ (ในเครื่องนี้เท่านั้น)"),
    ("That folder doesn't exist", "ไม่พบโฟลเดอร์นี้"),
    ("The defaults are good for most people.", "ค่าเริ่มต้นเหมาะกับคนส่วนใหญ่แล้ว"),
    ("This can take a moment on big folders.", "ถ้าโฟลเดอร์ใหญ่อาจใช้เวลาสักครู่"),
    ("Tidying up…", "กำลังจัดการ…"),
    ("Turn on lean builds", "เปิด lean builds"),
    ("Unlikely to be reused", "ไม่น่าจะได้ใช้อีก"),
    ("Unusual growth: +{} in a day", "โตผิดปกติ: +{} ในวันเดียว"),
    ("Up to", "สูงสุด"),
    ("Updating schedule…", "กำลังอัปเดตตารางเวลา…"),
    ("We found these folders. rtidy only looks inside them.", "เราพบโฟลเดอร์เหล่านี้ rtidy จะดูเฉพาะข้างในโฟลเดอร์เหล่านี้"),
    ("Welcome to rtidy", "ยินดีต้อนรับสู่ rtidy"),
    ("Where are your projects?", "โปรเจกต์ของคุณอยู่ที่ไหน?"),
    ("Where rtidy looks for Rust projects", "ที่ที่ rtidy จะค้นหาโปรเจกต์ Rust"),
    ("You can turn on daily clean-up later from the dashboard.", "เปิดการล้างรายวันทีหลังได้จากแดชบอร์ด"),
    ("You rarely build around {}:00", "คุณแทบไม่ build ช่วง {}:00"),
    ("You're all set", "พร้อมแล้ว"),
    ("Your next build of each project will take a little longer once; after that it's faster.", "build ครั้งถัดไปของแต่ละโปรเจกต์จะนานขึ้นนิดหน่อยแค่ครั้งเดียว หลังจากนั้นจะเร็วขึ้น"),
    ("at", "เวลา"),
    ("can be freed", "คืนพื้นที่ได้"),
    ("cap {} GB", "จำกัด {} GB"),
    ("couldn't set up daily clean-up ({})", "ตั้งการล้างรายวันไม่สำเร็จ ({})"),
    ("days", "วัน"),
    ("downloads", "ไฟล์ดาวน์โหลด"),
    ("e.g. {}. Their target/ folders are safe to remove.", "เช่น {} ลบโฟลเดอร์ target/ ของโปรเจกต์เหล่านี้ได้อย่างปลอดภัย"),
    ("full in ~{} days", "เต็มในอีก ~{} วัน"),
    ("if the computer is off, it runs at next start-up", "ถ้าเครื่องปิดอยู่ จะรันตอนเปิดเครื่องครั้งถัดไป"),
    ("learning…", "กำลังเรียนรู้…"),
    ("not set up", "ยังไม่ได้ตั้งค่า"),
    ("rtidy will keep things tidy from now on. You can close this window.", "ต่อจากนี้ rtidy จะดูแลให้เอง ปิดหน้าต่างนี้ได้เลย"),
    ("target/ folders", "โฟลเดอร์ target/"),
    ("{} build units have under {}% chance of being used in the next {} days. They go in the next clean-up.", "มี {} หน่วย build ที่มีโอกาสถูกใช้ไม่ถึง {}% ใน {} วันข้างหน้า จะถูกลบในรอบล้างถัดไป"),
    ("{} folders", "{} โฟลเดอร์"),
    ("{} leftover target/ folders ({})", "มีโฟลเดอร์ target/ ค้างอยู่ {} โฟลเดอร์ ({})"),
    ("{} of build artifacts you probably won't need", "ไฟล์ build {} ที่คุณน่าจะไม่ได้ใช้แล้ว"),
    ("{} outcomes learned. Runs on this computer only; nothing is uploaded.", "เรียนรู้แล้ว {} ผลลัพธ์ ประมวลผลในเครื่องนี้เท่านั้น ไม่มีการส่งข้อมูลออก"),
    ("{} projects you're unlikely to build soon ({})", "{} โปรเจกต์ที่คุณไม่น่าจะ build เร็วๆ นี้ ({})"),
    ("{} sets its own target-dir", "{} ตั้ง target-dir ของตัวเองไว้"),
    ("{} toolchains", "{} toolchain"),
    ("{}d ago", "{} วันก่อน"),
    ("~/.cargo downloads", "ไฟล์ดาวน์โหลดใน ~/.cargo"),
    ("…or type a folder path", "…หรือพิมพ์ path ของโฟลเดอร์"),
    ("Move rtidy to your Applications folder first, then open it from there — otherwise the daily clean-up can't find it.", "ย้าย rtidy ไปไว้ในโฟลเดอร์ Applications ก่อน แล้วเปิดจากตรงนั้น ไม่อย่างนั้นการล้างรายวันจะหาโปรแกรมไม่เจอ"),
    ("Insights", "ข้อมูลเชิงลึก"),
    ("Smart mode", "โหมดอัจฉริยะ"),
    ("Fix everything", "จัดการทั้งหมด"),
    ("Tidy up now", "จัดการเลย"),
    ("Rescan", "สแกนใหม่"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every t("…")/tf("…") string in the UI has a Thai translation with the same placeholders.
    #[test]
    fn all_strings_translated() {
        let sources = [
            include_str!("gui.rs"),
            include_str!("engine.rs"),
            include_str!("brain/mod.rs"),
        ];
        let map = &*THAI;
        let mut missing = Vec::new();
        for src in sources {
            for prefix in ["t(\"", "tf(\""] {
                for (i, _) in src.match_indices(prefix) {
                    // skip identifiers ending in t/tf (e.g. "set(", "left(")
                    let before = src[..i].chars().last().unwrap_or(' ');
                    if before.is_alphanumeric() || before == '_' {
                        continue;
                    }
                    let rest = &src[i + prefix.len()..];
                    let end = rest.find('"').unwrap();
                    let key = &rest[..end];
                    match map.get(key) {
                        None => missing.push(key.to_string()),
                        Some(th) if th.matches("{}").count() != key.matches("{}").count() => {
                            missing.push(format!("placeholder mismatch: {key}"))
                        }
                        _ => {}
                    }
                }
            }
        }
        assert!(missing.is_empty(), "missing Thai: {missing:#?}");
    }

    #[test]
    fn formats() {
        set(Lang::Th);
        assert_eq!(tf("Disk full in about {} days", &["9"]), "ดิสก์จะเต็มในอีกประมาณ 9 วัน");
        set(Lang::En);
        assert_eq!(tf("Freed {}, but: {}", &["1 GB", "x"]), "Freed 1 GB, but: x");
    }
}
