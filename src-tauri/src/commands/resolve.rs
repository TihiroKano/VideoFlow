//! `resolve_urls`：批量解析入口，按 URL 返回成功或分类失败，错误项不阻塞合法项。

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::core::model::ResolvedMedia;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveOutcome {
    pub url: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<ResolvedMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<AppError>,
}

const MAX_BATCH: usize = 50;

#[tauri::command]
pub async fn resolve_urls(
    state: State<'_, Arc<AppState>>,
    urls: Vec<String>,
) -> AppResult<Vec<ResolveOutcome>> {
    if urls.is_empty() {
        return Err(AppError::new("URL_EMPTY", "没有需要解析的链接", false));
    }

    state.prune_resolved();

    let mut outcomes = Vec::with_capacity(urls.len());

    for url in urls.into_iter().take(MAX_BATCH) {
        let trimmed = url.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        match crate::commands::resolve_media(&state, &trimmed, None).await {
            Ok(media) => {
                state.store_resolved(media.clone());
                outcomes.push(ResolveOutcome {
                    url: trimmed,
                    ok: true,
                    media: Some(media),
                    failure: None,
                });
            }
            Err(err) => {
                outcomes.push(ResolveOutcome {
                    url: trimmed,
                    ok: false,
                    media: None,
                    failure: Some(err),
                });
            }
        }
    }

    Ok(outcomes)
}