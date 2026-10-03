use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    path::PathBuf,
};

fn main() {
    embed_report_secrets();
    tauri_build::build()
}

/// Ключ бота для отчётов о проблемах и чат, куда они приходят.
///
/// Берутся из окружения сборки — в релизе это секреты GitHub, — а не из
/// исходников: репозиторий публичный. В бинарь кладутся под маской, чтобы
/// ключ не находился простым поиском строк. Не задано — отправка отчётов в
/// этой сборке выключена, всё остальное работает как обычно.
fn embed_report_secrets() {
    println!("cargo:rerun-if-env-changed=BRED_REPORT_TOKEN");
    println!("cargo:rerun-if-env-changed=BRED_REPORT_CHAT");

    let mut mask = [0u8; 32];
    for chunk in mask.chunks_mut(8) {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(std::process::id() as u64);
        chunk.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    let masked = |name: &str| -> Vec<u8> {
        std::env::var(name)
            .unwrap_or_default()
            .trim()
            .bytes()
            .zip(mask.iter().cycle())
            .map(|(byte, key)| byte ^ key)
            .collect()
    };

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR задаёт cargo"))
        .join("report_secrets.rs");
    let code = format!(
        "pub const MASK: [u8; 32] = {mask:?};\n\
         pub const TOKEN: &[u8] = &{token:?};\n\
         pub const CHAT: &[u8] = &{chat:?};\n",
        token = masked("BRED_REPORT_TOKEN"),
        chat = masked("BRED_REPORT_CHAT"),
    );
    std::fs::write(out, code).expect("секреты отчётов записаны");
}
