//! `yuki-tool languages` — 提出・ジェネレータで使う言語 ID の一覧。認証不要。

use anyhow::Result;

use crate::api::{YukicoderClient, DEFAULT_BASE_URL};

pub fn run(include_disabled: bool) -> Result<()> {
    let client = YukicoderClient::anonymous(DEFAULT_BASE_URL)?;

    for lang in client.languages()? {
        let enabled = lang.status.is_empty() || lang.status == "enable";
        if !enabled && !include_disabled {
            continue;
        }
        let suffix = if enabled {
            String::new()
        } else {
            format!(" [{}]", lang.status)
        };
        println!("{:<24} {} ({}){suffix}", lang.id, lang.name, lang.ver);
    }
    Ok(())
}
