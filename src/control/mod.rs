mod config;
mod execution;
mod failures;
mod resolver;

pub use config::load_control_config;
pub use execution::execute_control;
pub use failures::{
    build_failure_summary, build_failure_summary_payload, load_failure_state,
    maybe_consume_advisor_suggestion, prune_failure_state, record_control_failure,
    save_failure_state,
};
pub use resolver::{resolve_control_target, resolve_control_target_with_states};
