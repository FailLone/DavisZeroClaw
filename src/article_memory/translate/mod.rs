//! zh-CN translation worker. Delegates LLM calls to zeroclaw `/api/chat` via
//! a **private, non-exported** HTTP client (`remote_chat`). This privacy is
//! an intentional architectural choice — see the implementation plan at
//! `docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md` §"Anchor decisions"
//! A1/A3.

mod prompt;
mod remote_chat;
pub mod worker;

pub use worker::{run_one_cycle, TranslateWorker, TranslateWorkerDeps};
