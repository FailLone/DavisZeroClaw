use crate::app_config::{load_local_config, LocalConfig, MetricWeights, ModelProviderConfig};
use crate::ha_client::normalize_ha_url;
use crate::runtime_paths::RuntimePaths;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs::OpenOptions;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::time::Duration as StdDuration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RoutingProfile {
    HomeControl,
    GeneralQa,
    Research,
    StructuredLookup,
}

impl RoutingProfile {
    pub fn all() -> [Self; 4] {
        [
            Self::HomeControl,
            Self::GeneralQa,
            Self::Research,
            Self::StructuredLookup,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HomeControl => "home_control",
            Self::GeneralQa => "general_qa",
            Self::Research => "research",
            Self::StructuredLookup => "structured_lookup",
        }
    }

    fn weights<'a>(&self, config: &'a LocalConfig) -> &'a MetricWeights {
        match self {
            Self::HomeControl => &config.routing.profiles.home_control.weights,
            Self::GeneralQa => &config.routing.profiles.general_qa.weights,
            Self::Research => &config.routing.profiles.research.weights,
            Self::StructuredLookup => &config.routing.profiles.structured_lookup.weights,
        }
    }

    fn minimums(&self, config: &LocalConfig) -> (u8, u8) {
        let minimums = match self {
            Self::HomeControl => &config.routing.profiles.home_control.minimums,
            Self::GeneralQa => &config.routing.profiles.general_qa.minimums,
            Self::Research => &config.routing.profiles.research.minimums,
            Self::StructuredLookup => &config.routing.profiles.structured_lookup.minimums,
        };
        (minimums.task_success, minimums.safety)
    }

    fn max_fallbacks(&self, config: &LocalConfig) -> usize {
        match self {
            Self::HomeControl => config.routing.profiles.home_control.max_fallbacks,
            Self::GeneralQa => config.routing.profiles.general_qa.max_fallbacks,
            Self::Research => config.routing.profiles.research.max_fallbacks,
            Self::StructuredLookup => config.routing.profiles.structured_lookup.max_fallbacks,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricScore {
    pub task_success: u8,
    pub safety: u8,
    pub latency: u8,
    pub stability: u8,
    pub cost: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelScoreEntry {
    pub profile: RoutingProfile,
    pub provider: String,
    pub provider_alias: String,
    pub model: String,
    pub available: bool,
    pub total_score: f64,
    pub metrics: MetricScore,
    pub last_latency_ms: Option<u64>,
    pub checked_at: String,
    pub failure_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeScoreSignals>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RuntimeScoreSignals {
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub observed_requests: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub session_failures: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub tool_call_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub task_success_penalty: u8,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub safety_penalty: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeObservations {
    pub generated_at: String,
    pub window_start: String,
    #[serde(default)]
    pub model_costs: Vec<ModelCostObservation>,
    #[serde(default)]
    pub profile_observations: Vec<ProfileRuntimeObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCostObservation {
    pub model: String,
    pub request_count: u32,
    pub total_cost_usd: f64,
    pub avg_cost_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileRuntimeObservation {
    pub profile: RoutingProfile,
    pub provider: String,
    pub model: String,
    pub request_count: u32,
    pub failure_count: u32,
    pub tool_call_count: u32,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub task_success_penalty: u8,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub safety_penalty: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RouteHistoryEntry {
    time: String,
    trigger: String,
    plan_changed: bool,
    restart_requested: bool,
    restart_performed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlannedModel {
    pub provider: String,
    pub provider_alias: String,
    pub model: String,
    pub total_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlannedProfileRoute {
    pub profile: RoutingProfile,
    pub primary: PlannedModel,
    pub fallbacks: Vec<PlannedModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelRoutePlan {
    pub generated_at: String,
    pub default_profile: RoutingProfile,
    pub routes: Vec<PlannedProfileRoute>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingStatus {
    pub status: String,
    pub route_ready: bool,
    pub route_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug)]
struct RoutingState {
    status: RoutingStatus,
    plan: Option<ModelRoutePlan>,
    scorecard: Vec<ModelScoreEntry>,
    observations: Option<RuntimeObservations>,
    last_restart_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct ModelRoutingManager {
    paths: RuntimePaths,
    local_config: LocalConfig,
    state: Arc<RwLock<RoutingState>>,
}

impl ModelRoutingManager {
    pub fn spawn(paths: RuntimePaths, local_config: LocalConfig) -> Result<Arc<Self>> {
        let manager = Arc::new(Self {
            paths,
            local_config,
            state: Arc::new(RwLock::new(RoutingState {
                status: RoutingStatus {
                    status: "starting".to_string(),
                    route_ready: false,
                    route_version: 0,
                    last_updated_at: None,
                    last_error: None,
                },
                plan: None,
                scorecard: Vec::new(),
                observations: None,
                last_restart_at: None,
            })),
        });
        manager.refresh_once("initial", false)?;

        let worker = manager.clone();
        tokio::spawn(async move {
            let interval =
                StdDuration::from_secs(worker.local_config.routing.recompute_interval_minutes * 60);
            loop {
                tokio::time::sleep(interval).await;
                let _ = worker.refresh_once("periodic", true);
            }
        });

        Ok(manager)
    }

    #[cfg(test)]
    pub(crate) fn for_tests(paths: RuntimePaths, local_config: LocalConfig) -> Arc<Self> {
        Arc::new(Self {
            paths,
            local_config,
            state: Arc::new(RwLock::new(RoutingState {
                status: RoutingStatus {
                    status: "ready".to_string(),
                    route_ready: true,
                    route_version: 1,
                    last_updated_at: Some(crate::isoformat(Utc::now())),
                    last_error: None,
                },
                plan: None,
                scorecard: Vec::new(),
                observations: None,
                last_restart_at: None,
            })),
        })
    }

    pub async fn status(&self) -> RoutingStatus {
        self.state.read().unwrap().status.clone()
    }

    pub async fn plan(&self) -> Option<ModelRoutePlan> {
        self.state.read().unwrap().plan.clone()
    }

    pub async fn scorecard(&self) -> Vec<ModelScoreEntry> {
        self.state.read().unwrap().scorecard.clone()
    }

    pub async fn observations(&self) -> Option<RuntimeObservations> {
        self.state.read().unwrap().observations.clone()
    }

    fn refresh_once(&self, trigger: &str, allow_restart: bool) -> Result<()> {
        let previous_plan = self.current_plan().or_else(|| self.load_persisted_plan());
        let observations = collect_runtime_observations(&self.paths)?;
        let scorecard = build_scorecard(&self.local_config, &observations);

        self.persist_scorecard(&scorecard)?;
        self.persist_observations(&observations)?;

        match build_route_plan(&self.local_config, &scorecard) {
            Ok(plan) => {
                let plan_changed = previous_plan
                    .as_ref()
                    .map(|current| !same_route_plan(current, &plan))
                    .unwrap_or(true);
                let should_render = plan_changed || !self.paths.runtime_config_path().exists();
                if should_render {
                    self.render_runtime_config(&plan)?;
                }
                self.persist_plan(&plan)?;

                let (restart_requested, restart_performed, restart_error) =
                    if allow_restart && plan_changed {
                        self.maybe_restart_zeroclaw()?
                    } else {
                        (false, false, None)
                    };

                {
                    let mut state = self.state.write().unwrap();
                    let next_version = if plan_changed {
                        state.status.route_version.saturating_add(1).max(1)
                    } else {
                        state.status.route_version.max(1)
                    };
                    state.status = RoutingStatus {
                        status: "ready".to_string(),
                        route_ready: true,
                        route_version: next_version,
                        last_updated_at: Some(plan.generated_at.clone()),
                        last_error: restart_error.clone(),
                    };
                    if restart_performed {
                        state.last_restart_at = Some(Utc::now());
                    }
                    state.plan = Some(plan.clone());
                    state.scorecard = scorecard;
                    state.observations = Some(observations);
                }

                self.append_history(RouteHistoryEntry {
                    time: crate::isoformat(Utc::now()),
                    trigger: trigger.to_string(),
                    plan_changed,
                    restart_requested,
                    restart_performed,
                    error: restart_error,
                })?;
            }
            Err(err) => {
                let message = err.to_string();
                {
                    let mut state = self.state.write().unwrap();
                    let route_ready = state.plan.is_some();
                    state.status.status = if route_ready {
                        "degraded".to_string()
                    } else {
                        "error".to_string()
                    };
                    state.status.route_ready = route_ready;
                    state.status.last_error = Some(message.clone());
                    state.scorecard = scorecard;
                    state.observations = Some(observations);
                }

                self.append_history(RouteHistoryEntry {
                    time: crate::isoformat(Utc::now()),
                    trigger: format!("{trigger}_error"),
                    plan_changed: false,
                    restart_requested: false,
                    restart_performed: false,
                    error: Some(message),
                })?;
            }
        }

        Ok(())
    }

    fn current_plan(&self) -> Option<ModelRoutePlan> {
        self.state.read().unwrap().plan.clone()
    }

    fn load_persisted_plan(&self) -> Option<ModelRoutePlan> {
        let path = self.paths.model_route_plan_path();
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn persist_plan(&self, plan: &ModelRoutePlan) -> Result<()> {
        std::fs::create_dir_all(self.paths.state_dir())?;
        std::fs::write(
            self.paths.model_route_plan_path(),
            serde_json::to_vec_pretty(plan)?,
        )?;
        Ok(())
    }

    fn persist_scorecard(&self, scorecard: &[ModelScoreEntry]) -> Result<()> {
        std::fs::create_dir_all(self.paths.state_dir())?;
        std::fs::write(
            self.paths.model_scorecard_path(),
            serde_json::to_vec_pretty(scorecard)?,
        )?;
        Ok(())
    }

    fn persist_observations(&self, observations: &RuntimeObservations) -> Result<()> {
        std::fs::create_dir_all(self.paths.state_dir())?;
        std::fs::write(
            self.paths.model_runtime_observations_path(),
            serde_json::to_vec_pretty(observations)?,
        )?;
        Ok(())
    }

    fn append_history(&self, entry: RouteHistoryEntry) -> Result<()> {
        std::fs::create_dir_all(self.paths.state_dir())?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.paths.model_route_history_path())?;
        writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        Ok(())
    }

    fn render_runtime_config(&self, plan: &ModelRoutePlan) -> Result<()> {
        let template = std::fs::read_to_string(self.paths.config_template_path())
            .context("failed to read ZeroClaw config template")?;
        let default_route = plan
            .routes
            .iter()
            .find(|route| route.profile == RoutingProfile::GeneralQa)
            .ok_or_else(|| anyhow!("general_qa route missing from plan"))?;

        let rendered = template
            .replace(
                "__DAVIS_DEFAULT_PROVIDER__",
                &default_route.primary.provider_alias,
            )
            .replace("__DAVIS_DEFAULT_MODEL__", &default_route.primary.model)
            .replace("__DAVIS_DEFAULT_TEMPERATURE__", "0.3")
            .replace(
                "__DAVIS_IMESSAGE_CONFIG__",
                &render_imessage_config(&self.local_config),
            )
            .replace(
                "__DAVIS_WEBHOOK_SECRET_CONFIG__",
                &render_webhook_secret_config(&self.local_config),
            )
            .replace(
                "__DAVIS_MODEL_PROVIDERS__",
                &render_model_providers_config(&self.local_config),
            )
            .replace(
                "__DAVIS_QUERY_CLASSIFICATION_CONFIG__",
                &render_query_classification(),
            )
            .replace(
                "__DAVIS_MODEL_ROUTES_CONFIG__",
                &render_model_routes(plan, &self.local_config),
            )
            .replace("__DAVIS_MODEL_FALLBACKS__", &render_model_fallbacks(plan))
            .replace(
                "__DAVIS_FALLBACK_PROVIDERS__",
                &render_fallback_providers(plan),
            )
            .replace(
                "__DAVIS_HA_URL__",
                &toml_escape(
                    &normalize_ha_url(&self.local_config.home_assistant.url)
                        .map_err(anyhow::Error::msg)?,
                ),
            )
            .replace(
                "__DAVIS_HA_TOKEN__",
                &toml_escape(&self.local_config.home_assistant.token),
            );

        std::fs::create_dir_all(&self.paths.runtime_dir)?;
        std::fs::write(self.paths.runtime_config_path(), rendered)?;
        Ok(())
    }

    fn maybe_restart_zeroclaw(&self) -> Result<(bool, bool, Option<String>)> {
        let Some(pid) = read_pid(&self.paths.daemon_pid_path()) else {
            return Ok((
                true,
                false,
                Some("zeroclaw daemon is not running".to_string()),
            ));
        };

        let debounce = Duration::minutes(self.local_config.routing.restart_debounce_minutes as i64);
        if let Some(last_restart_at) = self.state.read().unwrap().last_restart_at {
            if Utc::now() - last_restart_at < debounce {
                return Ok((
                    true,
                    false,
                    Some("restart skipped by debounce window".to_string()),
                ));
            }
        }

        terminate_pid(pid);
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.paths.daemon_log_path())?;
        let log_file_err = log_file.try_clone()?;
        let child = Command::new("zeroclaw")
            .arg("daemon")
            .arg("--config-dir")
            .arg(&self.paths.runtime_dir)
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_file_err))
            .spawn()
            .context("failed to restart zeroclaw daemon")?;

        std::fs::write(self.paths.daemon_pid_path(), child.id().to_string())?;

        for _ in 0..20 {
            if service_ports_ready() {
                return Ok((true, true, None));
            }
            std::thread::sleep(StdDuration::from_millis(500));
        }

        Ok((
            true,
            false,
            Some("zeroclaw daemon did not become ready after restart".to_string()),
        ))
    }
}

pub fn zeroclaw_env_vars(config: &LocalConfig) -> Vec<(String, String)> {
    let mut exports = Vec::new();
    let mut seen = BTreeSet::new();
    for provider in &config.providers {
        for env_name in provider_api_key_env_names(&provider.name) {
            if seen.insert(env_name.clone()) {
                exports.push((env_name, provider.api_key.clone()));
            }
        }
    }
    exports
}

#[cfg_attr(not(test), allow(dead_code))]
fn build_declared_scorecard(config: &LocalConfig) -> Vec<ModelScoreEntry> {
    build_scorecard(config, &RuntimeObservations::empty())
}

fn build_scorecard(
    config: &LocalConfig,
    observations: &RuntimeObservations,
) -> Vec<ModelScoreEntry> {
    let costs_by_model = observations
        .model_costs
        .iter()
        .map(|entry| (entry.model.clone(), entry))
        .collect::<HashMap<_, _>>();
    let profile_observations = observations
        .profile_observations
        .iter()
        .map(|entry| {
            (
                (entry.profile, entry.provider.clone(), entry.model.clone()),
                entry,
            )
        })
        .collect::<HashMap<_, _>>();

    let mut entries = Vec::new();
    for (index, provider) in config.providers.iter().enumerate() {
        let alias = provider_alias(index, &provider.name);
        for model in &provider.allowed_models {
            for profile in RoutingProfile::all() {
                let mut metrics = MetricScore {
                    task_success: heuristic_task_success(&provider.name, model, profile),
                    safety: heuristic_safety(&provider.name, model, profile),
                    latency: heuristic_latency(&provider.name, model),
                    stability: heuristic_stability(&provider.name, model),
                    cost: heuristic_cost(&provider.name, model),
                };
                let runtime_observation = profile_observations
                    .get(&(profile, provider.name.clone(), model.clone()))
                    .copied();
                let cost_observation = costs_by_model.get(model).copied();
                apply_runtime_adjustments(
                    &mut metrics,
                    profile,
                    runtime_observation,
                    cost_observation,
                );

                let runtime = build_runtime_signals(runtime_observation, cost_observation);
                let failure_count = runtime_observation
                    .map(|entry| entry.failure_count)
                    .unwrap_or(0);
                let available = !runtime_observation
                    .map(should_temporarily_disable)
                    .unwrap_or(false);
                let total_score = weighted_score(&metrics, profile.weights(config));
                entries.push(ModelScoreEntry {
                    profile,
                    provider: provider.name.clone(),
                    provider_alias: alias.clone(),
                    model: model.clone(),
                    available,
                    total_score,
                    metrics,
                    last_latency_ms: runtime_observation.and_then(|entry| entry.avg_latency_ms),
                    checked_at: crate::isoformat(Utc::now()),
                    failure_count,
                    runtime,
                });
            }
        }
    }
    entries
}

fn build_route_plan(config: &LocalConfig, scorecard: &[ModelScoreEntry]) -> Result<ModelRoutePlan> {
    let mut routes = Vec::new();
    for profile in RoutingProfile::all() {
        let (min_task_success, min_safety) = profile.minimums(config);
        let mut candidates: Vec<&ModelScoreEntry> = scorecard
            .iter()
            .filter(|entry| {
                entry.profile == profile
                    && entry.available
                    && entry.metrics.task_success >= min_task_success
                    && entry.metrics.safety >= min_safety
            })
            .collect();
        candidates.sort_by(|left, right| {
            right
                .total_score
                .partial_cmp(&left.total_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.metrics.task_success.cmp(&left.metrics.task_success))
                .then_with(|| right.metrics.safety.cmp(&left.metrics.safety))
                .then_with(|| right.metrics.latency.cmp(&left.metrics.latency))
                .then_with(|| left.model.cmp(&right.model))
        });
        let primary = candidates
            .first()
            .ok_or_else(|| anyhow!("no viable model found for profile {}", profile.as_str()))?;
        let primary_model_plan = PlannedModel {
            provider: primary.provider.clone(),
            provider_alias: primary.provider_alias.clone(),
            model: primary.model.clone(),
            total_score: primary.total_score,
        };
        let primary_provider_alias = primary.provider_alias.clone();
        let primary_model = primary.model.clone();
        let fallbacks = candidates
            .into_iter()
            .filter(|entry| {
                entry.provider_alias != primary_provider_alias || entry.model != primary_model
            })
            .take(profile.max_fallbacks(config))
            .map(|entry| PlannedModel {
                provider: entry.provider.clone(),
                provider_alias: entry.provider_alias.clone(),
                model: entry.model.clone(),
                total_score: entry.total_score,
            })
            .collect();
        routes.push(PlannedProfileRoute {
            profile,
            primary: primary_model_plan,
            fallbacks,
        });
    }

    Ok(ModelRoutePlan {
        generated_at: crate::isoformat(Utc::now()),
        default_profile: RoutingProfile::GeneralQa,
        routes,
    })
}

impl RuntimeObservations {
    #[cfg_attr(not(test), allow(dead_code))]
    fn empty() -> Self {
        let now = Utc::now();
        Self {
            generated_at: crate::isoformat(now),
            window_start: crate::isoformat(now - Duration::hours(24)),
            model_costs: Vec::new(),
            profile_observations: Vec::new(),
        }
    }
}

#[derive(Default)]
struct CostAccumulator {
    request_count: u32,
    total_cost_usd: f64,
    last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct ProfileObservationAccumulator {
    request_count: u32,
    failure_count: u32,
    tool_call_count: u32,
    task_success_penalty: u32,
    safety_penalty: u32,
    latency_sum_ms: u64,
    latency_samples: u32,
    last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct TurnTraceObservation {
    prompt: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    tool_call_count: u32,
    had_failure: bool,
    task_success_penalty: u8,
    safety_penalty: u8,
    elapsed_ms: Option<u64>,
    last_seen_at: Option<DateTime<Utc>>,
}

fn collect_runtime_observations(paths: &RuntimePaths) -> Result<RuntimeObservations> {
    let now = Utc::now();
    let window_start = now - Duration::hours(24);
    let model_costs = collect_model_costs(paths, window_start)?;
    let profile_observations = collect_profile_observations(paths, window_start)?;

    Ok(RuntimeObservations {
        generated_at: crate::isoformat(now),
        window_start: crate::isoformat(window_start),
        model_costs,
        profile_observations,
    })
}

fn collect_model_costs(
    paths: &RuntimePaths,
    window_start: DateTime<Utc>,
) -> Result<Vec<ModelCostObservation>> {
    let mut costs = HashMap::<String, CostAccumulator>::new();
    let path = paths.workspace_costs_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = std::fs::read_to_string(path)?;
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let usage = value.get("usage").and_then(Value::as_object);
        let Some(usage) = usage else {
            continue;
        };
        let Some(model) = usage.get("model").and_then(Value::as_str) else {
            continue;
        };
        let Some(timestamp) = usage
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc)
        else {
            continue;
        };
        if timestamp < window_start {
            continue;
        }
        let cost_usd = usage.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
        let entry = costs.entry(model.to_string()).or_default();
        entry.request_count += 1;
        entry.total_cost_usd += cost_usd;
        entry.last_seen_at = Some(
            entry
                .last_seen_at
                .map(|current| current.max(timestamp))
                .unwrap_or(timestamp),
        );
    }

    let mut entries = costs
        .into_iter()
        .map(|(model, entry)| ModelCostObservation {
            model,
            request_count: entry.request_count,
            total_cost_usd: entry.total_cost_usd,
            avg_cost_usd: if entry.request_count == 0 {
                0.0
            } else {
                entry.total_cost_usd / f64::from(entry.request_count)
            },
            last_seen_at: entry.last_seen_at.map(crate::isoformat),
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.model.cmp(&right.model));
    Ok(entries)
}

fn collect_profile_observations(
    paths: &RuntimePaths,
    window_start: DateTime<Utc>,
) -> Result<Vec<ProfileRuntimeObservation>> {
    let Some(trace_path) = runtime_trace_path(paths) else {
        return Ok(Vec::new());
    };
    let raw = std::fs::read_to_string(trace_path)?;
    let mut turns = HashMap::<String, TurnTraceObservation>::new();

    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc);
        if timestamp.is_some_and(|ts| ts < window_start) {
            continue;
        }

        let Some(event_type) = value.get("event_type").and_then(Value::as_str) else {
            continue;
        };
        let turn_id = value
            .get("turn_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "channel:{}",
                    value.get("id").and_then(Value::as_str).unwrap_or("unknown")
                )
            });
        let turn = turns.entry(turn_id).or_default();
        if let Some(ts) = timestamp {
            turn.last_seen_at = Some(
                turn.last_seen_at
                    .map(|current| current.max(ts))
                    .unwrap_or(ts),
            );
        }

        if let Some(provider) = value.get("provider").and_then(Value::as_str) {
            turn.provider = Some(provider.to_string());
        }
        if let Some(model) = value.get("model").and_then(Value::as_str) {
            turn.model = Some(model.to_string());
        }

        match event_type {
            "channel_message_inbound" => {
                let payload = value.get("payload").and_then(Value::as_object);
                turn.prompt = payload
                    .and_then(|payload| payload.get("content_preview").and_then(Value::as_str))
                    .map(str::to_string)
                    .or_else(|| {
                        payload
                            .and_then(|payload| payload.get("content").and_then(Value::as_str))
                            .map(str::to_string)
                    });
            }
            "llm_response" => {
                if value.get("success").and_then(Value::as_bool) == Some(false) {
                    turn.had_failure = true;
                    turn.task_success_penalty = turn.task_success_penalty.saturating_add(8);
                }
                let payload = value.get("payload").and_then(Value::as_object);
                let parsed_calls = payload
                    .and_then(|payload| payload.get("parsed_tool_calls").and_then(Value::as_u64))
                    .unwrap_or(0) as u32;
                let native_calls = payload
                    .and_then(|payload| payload.get("native_tool_calls").and_then(Value::as_u64))
                    .unwrap_or(0) as u32;
                turn.tool_call_count = turn.tool_call_count.max(parsed_calls.max(native_calls));
                if turn.elapsed_ms.is_none() {
                    turn.elapsed_ms = payload
                        .and_then(|payload| payload.get("duration_ms").and_then(Value::as_u64));
                }
            }
            "tool_call_start" => {
                turn.tool_call_count += 1;
            }
            "tool_call_result" => {
                if value.get("success").and_then(Value::as_bool) == Some(false) {
                    turn.had_failure = true;
                    turn.task_success_penalty = turn.task_success_penalty.saturating_add(10);
                }
                if let Some(output) = value
                    .get("payload")
                    .and_then(Value::as_object)
                    .and_then(|payload| payload.get("output").and_then(Value::as_str))
                {
                    if output.contains("\"status\":\"failed\"")
                        || output.contains("\"status\":\"config_issue\"")
                        || output.contains("\"reason\":\"resolution_ambiguous\"")
                    {
                        turn.had_failure = true;
                        turn.task_success_penalty = turn.task_success_penalty.saturating_add(10);
                        turn.safety_penalty = turn.safety_penalty.saturating_add(12);
                    }
                }
            }
            "turn_final_response" => {
                if value.get("success").and_then(Value::as_bool) == Some(false) {
                    turn.had_failure = true;
                    turn.task_success_penalty = turn.task_success_penalty.saturating_add(8);
                }
            }
            "channel_message_outbound" => {
                let payload = value.get("payload").and_then(Value::as_object);
                turn.elapsed_ms = payload
                    .and_then(|payload| payload.get("elapsed_ms").and_then(Value::as_u64))
                    .or(turn.elapsed_ms);
                if value.get("success").and_then(Value::as_bool) == Some(false) {
                    turn.had_failure = true;
                    turn.task_success_penalty = turn.task_success_penalty.saturating_add(8);
                }
            }
            _ => {}
        }
    }

    let mut observations =
        HashMap::<(RoutingProfile, String, String), ProfileObservationAccumulator>::new();
    for turn in turns.into_values() {
        let (Some(provider), Some(model)) = (turn.provider, turn.model) else {
            continue;
        };
        let profile = infer_profile_from_message(turn.prompt.as_deref().unwrap_or_default());
        let entry = observations
            .entry((profile, provider.clone(), model.clone()))
            .or_default();
        entry.request_count += 1;
        if turn.had_failure {
            entry.failure_count += 1;
        }
        entry.tool_call_count += turn.tool_call_count;
        entry.task_success_penalty += u32::from(turn.task_success_penalty);
        if profile == RoutingProfile::HomeControl {
            entry.safety_penalty += u32::from(turn.safety_penalty);
        }
        if let Some(elapsed_ms) = turn.elapsed_ms {
            entry.latency_sum_ms += elapsed_ms;
            entry.latency_samples += 1;
        }
        if let Some(seen_at) = turn.last_seen_at {
            entry.last_seen_at = Some(
                entry
                    .last_seen_at
                    .map(|current| current.max(seen_at))
                    .unwrap_or(seen_at),
            );
        }
    }

    let mut entries = observations
        .into_iter()
        .map(
            |((profile, provider, model), entry)| ProfileRuntimeObservation {
                profile,
                provider,
                model,
                request_count: entry.request_count,
                failure_count: entry.failure_count,
                tool_call_count: entry.tool_call_count,
                task_success_penalty: entry.task_success_penalty.min(100) as u8,
                safety_penalty: entry.safety_penalty.min(100) as u8,
                avg_latency_ms: (entry.latency_samples > 0)
                    .then_some(entry.latency_sum_ms / u64::from(entry.latency_samples)),
                last_seen_at: entry.last_seen_at.map(crate::isoformat),
            },
        )
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.profile
            .cmp(&right.profile)
            .then_with(|| left.provider.cmp(&right.provider))
            .then_with(|| left.model.cmp(&right.model))
    });
    Ok(entries)
}

fn apply_runtime_adjustments(
    metrics: &mut MetricScore,
    profile: RoutingProfile,
    runtime_observation: Option<&ProfileRuntimeObservation>,
    cost_observation: Option<&ModelCostObservation>,
) {
    if let Some(runtime_observation) = runtime_observation {
        if let Some(avg_latency_ms) = runtime_observation.avg_latency_ms {
            metrics.latency = latency_score_from_ms(avg_latency_ms);
        }
        metrics.task_success = metrics
            .task_success
            .saturating_sub(runtime_observation.task_success_penalty)
            .saturating_sub((runtime_observation.failure_count.min(5) as u8).saturating_mul(4));
        metrics.safety = metrics
            .safety
            .saturating_sub(runtime_observation.safety_penalty);
        metrics.stability = observed_stability_score(runtime_observation);

        if profile == RoutingProfile::HomeControl
            && runtime_observation.tool_call_count > 0
            && runtime_observation.failure_count == 0
        {
            metrics.task_success = metrics.task_success.saturating_add(2).min(100);
            metrics.safety = metrics.safety.saturating_add(2).min(100);
        }
    }

    if let Some(cost_observation) = cost_observation {
        metrics.cost = cost_score_from_avg_usd(cost_observation.avg_cost_usd);
    }
}

fn build_runtime_signals(
    runtime_observation: Option<&ProfileRuntimeObservation>,
    cost_observation: Option<&ModelCostObservation>,
) -> Option<RuntimeScoreSignals> {
    match (runtime_observation, cost_observation) {
        (None, None) => None,
        (runtime_observation, cost_observation) => Some(RuntimeScoreSignals {
            observed_requests: runtime_observation
                .map(|entry| entry.request_count)
                .unwrap_or(0),
            session_failures: runtime_observation
                .map(|entry| entry.failure_count)
                .unwrap_or(0),
            tool_call_count: runtime_observation
                .map(|entry| entry.tool_call_count)
                .unwrap_or(0),
            avg_cost_usd: cost_observation.map(|entry| entry.avg_cost_usd),
            task_success_penalty: runtime_observation
                .map(|entry| entry.task_success_penalty)
                .unwrap_or(0),
            safety_penalty: runtime_observation
                .map(|entry| entry.safety_penalty)
                .unwrap_or(0),
        }),
    }
}

fn should_temporarily_disable(observation: &ProfileRuntimeObservation) -> bool {
    observation.request_count >= 3
        && observation.failure_count >= 3
        && observation.failure_count >= observation.request_count.saturating_sub(1)
}

fn observed_stability_score(observation: &ProfileRuntimeObservation) -> u8 {
    if observation.request_count == 0 {
        return 80;
    }
    let failure_ratio = observation.failure_count as f64 / observation.request_count as f64;
    ((1.0 - failure_ratio) * 100.0).round().clamp(0.0, 100.0) as u8
}

fn latency_score_from_ms(avg_latency_ms: u64) -> u8 {
    match avg_latency_ms {
        0..=1200 => 98,
        1201..=1800 => 94,
        1801..=2500 => 90,
        2501..=4000 => 82,
        4001..=6000 => 72,
        6001..=9000 => 58,
        _ => 42,
    }
}

fn cost_score_from_avg_usd(avg_cost_usd: f64) -> u8 {
    if avg_cost_usd <= 0.01 {
        95
    } else if avg_cost_usd <= 0.03 {
        85
    } else if avg_cost_usd <= 0.06 {
        72
    } else if avg_cost_usd <= 0.10 {
        55
    } else if avg_cost_usd <= 0.20 {
        35
    } else {
        18
    }
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn weighted_score(metrics: &MetricScore, weights: &MetricWeights) -> f64 {
    f64::from(metrics.task_success) * weights.task_success
        + f64::from(metrics.safety) * weights.safety
        + f64::from(metrics.latency) * weights.latency
        + f64::from(metrics.stability) * weights.stability
        + f64::from(metrics.cost) * weights.cost
}

fn heuristic_task_success(provider: &str, model: &str, profile: RoutingProfile) -> u8 {
    let name = format!("{provider}/{model}").to_lowercase();
    let mut score = if name.contains("claude-opus") {
        96
    } else if name.contains("claude-sonnet") || name.contains("gpt-5") {
        93
    } else if name.contains("gpt-4.1") || name.contains("gpt-4o") {
        89
    } else if name.contains("deepseek-reasoner") || name.contains("deepseek-r1") {
        90
    } else if name.contains("deepseek-chat") || name.contains("deepseek-v3") {
        84
    } else if name.contains("qwen-max") || name.contains("qwen-plus") {
        84
    } else if name.contains("moonshot") || name.contains("kimi") {
        81
    } else if name.contains("glm") {
        80
    } else {
        72
    };
    score += match profile {
        RoutingProfile::HomeControl => 2,
        RoutingProfile::Research if name.contains("reasoner") || name.contains("opus") => 4,
        RoutingProfile::StructuredLookup => 1,
        _ => 0,
    };
    score.clamp(0, 100)
}

fn heuristic_safety(provider: &str, model: &str, profile: RoutingProfile) -> u8 {
    let name = format!("{provider}/{model}").to_lowercase();
    let mut score = if name.contains("claude") || name.contains("gpt") {
        92
    } else if name.contains("deepseek") || name.contains("qwen") {
        84
    } else if name.contains("moonshot") || name.contains("kimi") || name.contains("glm") {
        78
    } else {
        70
    };
    if matches!(profile, RoutingProfile::HomeControl) {
        score += 4;
    }
    score.clamp(0, 100)
}

fn heuristic_cost(provider: &str, model: &str) -> u8 {
    let name = format!("{provider}/{model}").to_lowercase();
    if name.contains("openrouter") || name.contains("deepseek") || name.contains("siliconflow") {
        85
    } else if name.contains("qwen") || name.contains("moonshot") || name.contains("glm") {
        72
    } else if name.contains("gpt-5") || name.contains("claude-opus") {
        25
    } else if name.contains("gpt") || name.contains("claude") {
        40
    } else {
        55
    }
}

fn heuristic_latency(provider: &str, model: &str) -> u8 {
    let name = format!("{provider}/{model}").to_lowercase();
    if name.contains("deepseek-chat") || name.contains("deepseek-v3") {
        88
    } else if name.contains("qwen-plus") || name.contains("qwen-max") {
        84
    } else if name.contains("gpt-4o") || name.contains("gpt-4.1") {
        78
    } else if name.contains("claude-sonnet") || name.contains("moonshot") || name.contains("kimi") {
        70
    } else if name.contains("gpt-5") || name.contains("claude-opus") {
        62
    } else if name.contains("reasoner") || name.contains("r1") {
        48
    } else if name.contains("glm") {
        68
    } else {
        64
    }
}

fn heuristic_stability(provider: &str, model: &str) -> u8 {
    let name = format!("{provider}/{model}").to_lowercase();
    if name.contains("gpt") || name.contains("claude") {
        95
    } else if name.contains("openrouter") {
        90
    } else if name.contains("deepseek") || name.contains("qwen") {
        88
    } else if name.contains("moonshot") || name.contains("kimi") || name.contains("glm") {
        82
    } else {
        78
    }
}

fn infer_profile_from_message(message: &str) -> RoutingProfile {
    let normalized = message.to_lowercase();
    if contains_any(
        &normalized,
        &["快递", "物流", "运单", "单号", "顺丰", "京东快递", "圆通"],
    ) {
        RoutingProfile::StructuredLookup
    } else if contains_any(
        &normalized,
        &[
            "为什么",
            "原因",
            "谁",
            "昨晚",
            "之前",
            "历史",
            "记录",
            "研究",
            "分析",
            "建议",
            "怎么优化",
            "调查",
            "最近",
        ],
    ) {
        RoutingProfile::Research
    } else if contains_any(
        &normalized,
        &[
            "打开", "关闭", "开灯", "关灯", "亮度", "调亮", "调暗", "空调", "风扇", "窗帘", "插座",
            "开关", "灯带", "主灯",
        ],
    ) {
        RoutingProfile::HomeControl
    } else {
        RoutingProfile::GeneralQa
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn runtime_trace_path(paths: &RuntimePaths) -> Option<std::path::PathBuf> {
    let primary = paths.zeroclaw_runtime_trace_path();
    if primary.exists()
        && std::fs::metadata(&primary)
            .map(|meta| meta.len() > 0)
            .unwrap_or(false)
    {
        return Some(primary);
    }
    let fallback = paths
        .workspace_dir()
        .join("state")
        .join("runtime-trace.jsonl");
    if fallback.exists() {
        Some(fallback)
    } else {
        None
    }
}

fn same_route_plan(left: &ModelRoutePlan, right: &ModelRoutePlan) -> bool {
    left.default_profile == right.default_profile && left.routes == right.routes
}

fn read_pid(path: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

fn terminate_pid(pid: u32) {
    let _ = Command::new("kill").arg(pid.to_string()).status();
    for _ in 0..10 {
        let status = Command::new("kill").arg("-0").arg(pid.to_string()).status();
        if !matches!(status, Ok(exit) if exit.success()) {
            return;
        }
        std::thread::sleep(StdDuration::from_millis(200));
    }
}

fn service_ports_ready() -> bool {
    std::net::TcpStream::connect("127.0.0.1:3000").is_ok()
        && std::net::TcpStream::connect("127.0.0.1:3001").is_ok()
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn provider_alias(index: usize, provider_name: &str) -> String {
    let sanitized: String = provider_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("davis_{}_{}", index + 1, sanitized)
}

fn provider_config_for_alias<'a>(
    config: &'a LocalConfig,
    target_provider_alias: &str,
) -> Option<&'a ModelProviderConfig> {
    config
        .providers
        .iter()
        .enumerate()
        .find_map(|(index, provider)| {
            (target_provider_alias == provider_alias(index, &provider.name)).then_some(provider)
        })
}

fn render_imessage_config(config: &LocalConfig) -> String {
    let contacts = config
        .imessage
        .allowed_contacts
        .iter()
        .map(|item| format!("\"{}\"", toml_escape(item)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[channels_config.imessage]\nallowed_contacts = [{contacts}]\n")
}

fn render_webhook_secret_config(config: &LocalConfig) -> String {
    if config.webhook.secret.trim().is_empty() {
        "# secret = \"replace-with-your-webhook-secret\"".to_string()
    } else {
        format!("secret = \"{}\"", toml_escape(config.webhook.secret.trim()))
    }
}

fn render_model_providers_config(config: &LocalConfig) -> String {
    let mut blocks = Vec::new();
    for (index, provider) in config.providers.iter().enumerate() {
        let alias = provider_alias(index, &provider.name);
        let mut block = format!(
            "[model_providers.{alias}]\nname = \"{}\"\n",
            toml_escape(&provider.name)
        );
        if !provider.base_url.trim().is_empty() {
            block.push_str(&format!(
                "base_url = \"{}\"\n",
                toml_escape(provider.base_url.trim())
            ));
        }
        blocks.push(block);
    }
    blocks.join("\n")
}

fn render_query_classification() -> String {
    let rules = [
        (
            "structured_lookup",
            vec!["快递", "物流", "运单", "单号", "顺丰", "京东快递", "圆通"],
            40,
        ),
        (
            "research",
            vec![
                "为什么",
                "原因",
                "谁",
                "昨晚",
                "之前",
                "历史",
                "记录",
                "研究",
                "分析",
                "建议",
                "怎么优化",
            ],
            30,
        ),
        (
            "home_control",
            vec![
                "打开", "关闭", "开灯", "关灯", "亮度", "调亮", "调暗", "空调", "风扇", "窗帘",
                "插座", "开关", "灯带", "主灯",
            ],
            20,
        ),
    ];
    let mut output = String::from("[query_classification]\nenabled = true\n");
    for (hint, keywords, priority) in rules {
        let rendered_keywords = keywords
            .into_iter()
            .map(|item| format!("\"{}\"", toml_escape(item)))
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str("\n[[query_classification.rules]]\n");
        output.push_str(&format!("hint = \"{hint}\"\n"));
        output.push_str(&format!("keywords = [{rendered_keywords}]\n"));
        output.push_str(&format!("priority = {priority}\n"));
    }
    output
}

fn render_model_routes(plan: &ModelRoutePlan, config: &LocalConfig) -> String {
    let mut blocks = Vec::new();
    for route in &plan.routes {
        if let Some(provider) = provider_config_for_alias(config, &route.primary.provider_alias) {
            blocks.push(format!(
                "[[model_routes]]\nhint = \"{}\"\nprovider = \"{}\"\nmodel = \"{}\"\napi_key = \"{}\"\n",
                route.profile.as_str(),
                route.primary.provider,
                toml_escape(&route.primary.model),
                toml_escape(&provider.api_key)
            ));
        }
    }
    blocks.join("\n")
}

fn render_model_fallbacks(plan: &ModelRoutePlan) -> String {
    let mut rendered_keys = BTreeSet::new();
    let mut output = String::from("[reliability.model_fallbacks]\n");
    for route in &plan.routes {
        if route.fallbacks.is_empty() || !rendered_keys.insert(route.primary.model.clone()) {
            continue;
        }
        let fallback_models = route
            .fallbacks
            .iter()
            .map(|candidate| format!("\"{}\"", toml_escape(&candidate.model)))
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!(
            "\"{}\" = [{}]\n",
            toml_escape(&route.primary.model),
            fallback_models
        ));
    }
    output
}

fn render_fallback_providers(plan: &ModelRoutePlan) -> String {
    let Some(default_route) = plan
        .routes
        .iter()
        .find(|route| route.profile == plan.default_profile)
    else {
        return "[]".to_string();
    };

    let mut seen = BTreeSet::new();
    let providers = default_route
        .fallbacks
        .iter()
        .filter(|candidate| candidate.provider != default_route.primary.provider)
        .filter_map(|candidate| {
            seen.insert(candidate.provider.clone())
                .then(|| format!("\"{}\"", toml_escape(&candidate.provider)))
        })
        .collect::<Vec<_>>()
        .join(", ");

    format!("[{providers}]")
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn provider_api_key_env_names(provider_name: &str) -> Vec<String> {
    let normalized = provider_name
        .trim()
        .to_ascii_uppercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();

    match provider_name.trim().to_ascii_lowercase().as_str() {
        "qwen" | "dashscope" => vec!["DASHSCOPE_API_KEY".to_string(), "QWEN_API_KEY".to_string()],
        "moonshot" | "kimi" => vec!["MOONSHOT_API_KEY".to_string(), "KIMI_API_KEY".to_string()],
        "glm" | "zhipu" => vec!["GLM_API_KEY".to_string(), "ZHIPU_API_KEY".to_string()],
        "doubao" | "ark" | "volcengine" => vec![
            "DOUBAO_API_KEY".to_string(),
            "ARK_API_KEY".to_string(),
            "VOLCENGINE_API_KEY".to_string(),
        ],
        _ if normalized.is_empty() => Vec::new(),
        _ => vec![format!("{normalized}_API_KEY")],
    }
}

pub fn check_local_config(paths: &RuntimePaths) -> Result<LocalConfig> {
    load_local_config(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::LocalConfig;
    use std::fs;
    use std::time::Duration;

    fn sample_config() -> LocalConfig {
        toml::from_str(
            r#"
[home_assistant]
url = "http://127.0.0.1:8123/api/mcp"
token = "token"

[imessage]
allowed_contacts = ["+8613800138000"]

[[providers]]
name = "openrouter"
api_key = "key-1"
base_url = ""
allowed_models = ["openai/gpt-4o", "anthropic/claude-sonnet-4.6"]

[[providers]]
name = "deepseek"
api_key = "key-2"
base_url = ""
allowed_models = ["deepseek-chat"]

[routing]
recompute_interval_minutes = 30
restart_debounce_minutes = 10

[routing.profiles.home_control]
weights = { task_success = 0.45, safety = 0.30, stability = 0.15, latency = 0.08, cost = 0.02 }
minimums = { task_success = 80, safety = 90 }
max_fallbacks = 2

[routing.profiles.general_qa]
weights = { task_success = 0.42, latency = 0.28, stability = 0.15, safety = 0.10, cost = 0.05 }
minimums = { task_success = 60, safety = 40 }
max_fallbacks = 2

[routing.profiles.research]
weights = { task_success = 0.50, stability = 0.20, latency = 0.15, safety = 0.10, cost = 0.05 }
minimums = { task_success = 70, safety = 50 }
max_fallbacks = 2

[routing.profiles.structured_lookup]
weights = { task_success = 0.40, latency = 0.25, stability = 0.20, safety = 0.10, cost = 0.05 }
minimums = { task_success = 75, safety = 60 }
max_fallbacks = 2
"#,
        )
        .unwrap()
    }

    fn mock_score_entry(
        profile: RoutingProfile,
        provider: &str,
        provider_alias: &str,
        model: &str,
        available: bool,
        total_score: f64,
        task_success: u8,
        safety: u8,
        latency: u8,
        stability: u8,
        cost: u8,
    ) -> ModelScoreEntry {
        ModelScoreEntry {
            profile,
            provider: provider.into(),
            provider_alias: provider_alias.into(),
            model: model.into(),
            available,
            total_score,
            metrics: MetricScore {
                task_success,
                safety,
                latency,
                stability,
                cost,
            },
            last_latency_ms: None,
            checked_at: crate::isoformat(Utc::now()),
            failure_count: u32::from(!available),
            runtime: None,
        }
    }

    fn test_runtime_paths(test_name: &str) -> RuntimePaths {
        let suffix = Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_else(|| Utc::now().timestamp_micros() * 1000);
        let root = std::env::temp_dir().join(format!("davis-routing-{test_name}-{suffix}"));
        let paths = RuntimePaths {
            repo_root: root.clone(),
            runtime_dir: root.join(".runtime").join("davis"),
        };
        fs::create_dir_all(paths.state_dir()).unwrap();
        paths
    }

    #[test]
    fn build_route_plan_includes_cross_provider_fallbacks() {
        let config = sample_config();
        let scorecard = vec![
            ModelScoreEntry {
                profile: RoutingProfile::GeneralQa,
                provider: "openrouter".into(),
                provider_alias: "davis_1_openrouter".into(),
                model: "openai/gpt-4o".into(),
                available: true,
                total_score: 90.0,
                metrics: MetricScore {
                    task_success: 90,
                    safety: 92,
                    latency: 80,
                    stability: 90,
                    cost: 60,
                },
                last_latency_ms: Some(1200),
                checked_at: crate::isoformat(Utc::now()),
                failure_count: 0,
                runtime: None,
            },
            ModelScoreEntry {
                profile: RoutingProfile::GeneralQa,
                provider: "openrouter".into(),
                provider_alias: "davis_1_openrouter".into(),
                model: "anthropic/claude-sonnet-4.6".into(),
                available: true,
                total_score: 88.0,
                metrics: MetricScore {
                    task_success: 93,
                    safety: 95,
                    latency: 70,
                    stability: 90,
                    cost: 40,
                },
                last_latency_ms: Some(1700),
                checked_at: crate::isoformat(Utc::now()),
                failure_count: 0,
                runtime: None,
            },
            ModelScoreEntry {
                profile: RoutingProfile::GeneralQa,
                provider: "deepseek".into(),
                provider_alias: "davis_2_deepseek".into(),
                model: "deepseek-chat".into(),
                available: true,
                total_score: 82.0,
                metrics: MetricScore {
                    task_success: 84,
                    safety: 84,
                    latency: 85,
                    stability: 90,
                    cost: 85,
                },
                last_latency_ms: Some(900),
                checked_at: crate::isoformat(Utc::now()),
                failure_count: 0,
                runtime: None,
            },
            ModelScoreEntry {
                profile: RoutingProfile::HomeControl,
                provider: "openrouter".into(),
                provider_alias: "davis_1_openrouter".into(),
                model: "openai/gpt-4o".into(),
                available: true,
                total_score: 91.0,
                metrics: MetricScore {
                    task_success: 90,
                    safety: 93,
                    latency: 80,
                    stability: 90,
                    cost: 60,
                },
                last_latency_ms: Some(1200),
                checked_at: crate::isoformat(Utc::now()),
                failure_count: 0,
                runtime: None,
            },
            ModelScoreEntry {
                profile: RoutingProfile::HomeControl,
                provider: "deepseek".into(),
                provider_alias: "davis_2_deepseek".into(),
                model: "deepseek-chat".into(),
                available: true,
                total_score: 85.0,
                metrics: MetricScore {
                    task_success: 82,
                    safety: 91,
                    latency: 85,
                    stability: 90,
                    cost: 85,
                },
                last_latency_ms: Some(900),
                checked_at: crate::isoformat(Utc::now()),
                failure_count: 0,
                runtime: None,
            },
            ModelScoreEntry {
                profile: RoutingProfile::Research,
                provider: "openrouter".into(),
                provider_alias: "davis_1_openrouter".into(),
                model: "anthropic/claude-sonnet-4.6".into(),
                available: true,
                total_score: 94.0,
                metrics: MetricScore {
                    task_success: 95,
                    safety: 95,
                    latency: 70,
                    stability: 90,
                    cost: 40,
                },
                last_latency_ms: Some(1700),
                checked_at: crate::isoformat(Utc::now()),
                failure_count: 0,
                runtime: None,
            },
            ModelScoreEntry {
                profile: RoutingProfile::StructuredLookup,
                provider: "deepseek".into(),
                provider_alias: "davis_2_deepseek".into(),
                model: "deepseek-chat".into(),
                available: true,
                total_score: 87.0,
                metrics: MetricScore {
                    task_success: 85,
                    safety: 80,
                    latency: 85,
                    stability: 90,
                    cost: 85,
                },
                last_latency_ms: Some(900),
                checked_at: crate::isoformat(Utc::now()),
                failure_count: 0,
                runtime: None,
            },
        ];

        let plan = build_route_plan(&config, &scorecard).unwrap();
        let general = plan
            .routes
            .iter()
            .find(|route| route.profile == RoutingProfile::GeneralQa)
            .unwrap();
        assert_eq!(general.primary.model, "openai/gpt-4o");
        assert_eq!(general.fallbacks[0].provider_alias, "davis_1_openrouter");
        assert_eq!(general.fallbacks[0].model, "anthropic/claude-sonnet-4.6");
        assert_eq!(general.fallbacks[1].provider_alias, "davis_2_deepseek");
        assert_eq!(general.fallbacks[1].model, "deepseek-chat");
    }

    #[test]
    fn zeroclaw_env_vars_export_provider_keys() {
        let config = sample_config();
        let exports = zeroclaw_env_vars(&config);
        assert!(exports.contains(&("OPENROUTER_API_KEY".to_string(), "key-1".to_string())));
        assert!(exports.contains(&("DEEPSEEK_API_KEY".to_string(), "key-2".to_string())));
    }

    #[test]
    fn render_fallback_providers_uses_default_profile_cross_provider_order() {
        let plan = ModelRoutePlan {
            generated_at: crate::isoformat(Utc::now()),
            default_profile: RoutingProfile::GeneralQa,
            routes: vec![PlannedProfileRoute {
                profile: RoutingProfile::GeneralQa,
                primary: PlannedModel {
                    provider: "openrouter".into(),
                    provider_alias: "davis_1_openrouter".into(),
                    model: "openai/gpt-4o".into(),
                    total_score: 90.0,
                },
                fallbacks: vec![
                    PlannedModel {
                        provider: "openrouter".into(),
                        provider_alias: "davis_1_openrouter".into(),
                        model: "anthropic/claude-sonnet-4.6".into(),
                        total_score: 88.0,
                    },
                    PlannedModel {
                        provider: "deepseek".into(),
                        provider_alias: "davis_2_deepseek".into(),
                        model: "deepseek-chat".into(),
                        total_score: 82.0,
                    },
                    PlannedModel {
                        provider: "siliconflow".into(),
                        provider_alias: "davis_3_siliconflow".into(),
                        model: "deepseek-ai/DeepSeek-V3".into(),
                        total_score: 81.0,
                    },
                ],
            }],
        };

        assert_eq!(
            render_fallback_providers(&plan),
            "[\"deepseek\", \"siliconflow\"]"
        );
    }

    #[test]
    fn render_model_routes_uses_provider_name_not_alias() {
        let config = sample_config();
        let plan = ModelRoutePlan {
            generated_at: crate::isoformat(Utc::now()),
            default_profile: RoutingProfile::GeneralQa,
            routes: vec![
                PlannedProfileRoute {
                    profile: RoutingProfile::HomeControl,
                    primary: PlannedModel {
                        provider: "openrouter".into(),
                        provider_alias: "davis_1_openrouter".into(),
                        model: "openai/gpt-4o".into(),
                        total_score: 94.0,
                    },
                    fallbacks: vec![],
                },
                PlannedProfileRoute {
                    profile: RoutingProfile::GeneralQa,
                    primary: PlannedModel {
                        provider: "deepseek".into(),
                        provider_alias: "davis_2_deepseek".into(),
                        model: "deepseek-chat".into(),
                        total_score: 88.0,
                    },
                    fallbacks: vec![],
                },
            ],
        };

        let rendered = render_model_routes(&plan, &config);
        assert!(rendered.contains("hint = \"home_control\"\nprovider = \"openrouter\""));
        assert!(rendered.contains("hint = \"general_qa\"\nprovider = \"deepseek\""));
        assert!(!rendered.contains("provider = \"davis_1_openrouter\""));
    }

    #[test]
    fn build_declared_scorecard_marks_models_available_without_runtime_probe() {
        let config = sample_config();
        let scorecard = build_declared_scorecard(&config);
        let general = scorecard
            .iter()
            .find(|entry| {
                entry.profile == RoutingProfile::GeneralQa
                    && entry.provider == "openrouter"
                    && entry.model == "openai/gpt-4o"
            })
            .unwrap();

        assert!(general.available);
        assert!(general.total_score > 0.0);
        assert_eq!(general.failure_count, 0);
    }

    #[test]
    fn build_scorecard_uses_runtime_observations_to_switch_general_qa() {
        let mut config = sample_config();
        config.providers[1]
            .allowed_models
            .push("deepseek-reasoner".to_string());

        let observations = RuntimeObservations {
            generated_at: crate::isoformat(Utc::now()),
            window_start: crate::isoformat(Utc::now() - chrono::Duration::hours(24)),
            model_costs: vec![],
            profile_observations: vec![
                ProfileRuntimeObservation {
                    profile: RoutingProfile::GeneralQa,
                    provider: "openrouter".into(),
                    model: "openai/gpt-4o".into(),
                    request_count: 4,
                    failure_count: 4,
                    tool_call_count: 0,
                    task_success_penalty: 20,
                    safety_penalty: 0,
                    avg_latency_ms: Some(6200),
                    last_seen_at: None,
                },
                ProfileRuntimeObservation {
                    profile: RoutingProfile::GeneralQa,
                    provider: "deepseek".into(),
                    model: "deepseek-chat".into(),
                    request_count: 6,
                    failure_count: 0,
                    tool_call_count: 0,
                    task_success_penalty: 0,
                    safety_penalty: 0,
                    avg_latency_ms: Some(1400),
                    last_seen_at: None,
                },
            ],
        };

        let scorecard = build_scorecard(&config, &observations);
        let plan = build_route_plan(&config, &scorecard).unwrap();
        let general = plan
            .routes
            .iter()
            .find(|route| route.profile == RoutingProfile::GeneralQa)
            .unwrap();
        assert_eq!(general.primary.provider, "deepseek");
        assert_eq!(general.primary.model, "deepseek-chat");
        assert!(scorecard.iter().any(|entry| {
            entry.profile == RoutingProfile::GeneralQa
                && entry.provider == "openrouter"
                && entry.model == "openai/gpt-4o"
                && !entry.available
        }));
    }

    #[test]
    fn collect_runtime_observations_reads_trace_and_costs() {
        let paths = test_runtime_paths("runtime-observations");
        let now = Utc::now();
        let ts1 = crate::isoformat(now - chrono::Duration::minutes(3));
        let ts2 = crate::isoformat(now - chrono::Duration::minutes(2));
        let ts3 = crate::isoformat(now - chrono::Duration::minutes(1));
        fs::create_dir_all(paths.workspace_costs_path().parent().unwrap()).unwrap();
        fs::create_dir_all(paths.workspace_dir().join("state")).unwrap();
        fs::write(
            paths.workspace_costs_path(),
            format!(
                "{{\"usage\":{{\"model\":\"openai/gpt-4o\",\"cost_usd\":0.08,\"timestamp\":\"{ts3}\"}}}}\n"
            ),
        )
        .unwrap();
        fs::write(
            paths.workspace_dir().join("state").join("runtime-trace.jsonl"),
            format!(
                "{{\"timestamp\":\"{}\",\"event_type\":\"channel_message_inbound\",\"turn_id\":\"turn-1\",\"payload\":{{\"content_preview\":\"请把书房灯带打开一下\"}}}}\n\
                 {{\"timestamp\":\"{}\",\"event_type\":\"llm_response\",\"turn_id\":\"turn-1\",\"provider\":\"openrouter\",\"model\":\"openai/gpt-4o\",\"success\":true,\"payload\":{{\"duration_ms\":1800,\"parsed_tool_calls\":1}}}}\n\
                 {{\"timestamp\":\"{}\",\"event_type\":\"tool_call_result\",\"turn_id\":\"turn-1\",\"provider\":\"openrouter\",\"model\":\"openai/gpt-4o\",\"success\":true,\"payload\":{{\"output\":\"{{\\\"status\\\":\\\"success\\\"}}\"}}}}\n\
                 {{\"timestamp\":\"{}\",\"event_type\":\"channel_message_outbound\",\"turn_id\":\"turn-1\",\"provider\":\"openrouter\",\"model\":\"openai/gpt-4o\",\"success\":true,\"payload\":{{\"elapsed_ms\":2400}}}}\n",
                ts1, ts1, ts2, ts3
            ),
        )
        .unwrap();

        let observations = collect_runtime_observations(&paths).unwrap();
        assert_eq!(observations.model_costs.len(), 1);
        assert_eq!(observations.model_costs[0].model, "openai/gpt-4o");
        assert_eq!(observations.profile_observations.len(), 1);
        assert_eq!(
            observations.profile_observations[0].profile,
            RoutingProfile::HomeControl
        );
        assert_eq!(observations.profile_observations[0].request_count, 1);
        assert_eq!(observations.profile_observations[0].tool_call_count, 1);
        assert_eq!(
            observations.profile_observations[0].avg_latency_ms,
            Some(2400)
        );
    }

    #[test]
    fn home_control_can_switch_models_within_same_provider() {
        let config = sample_config();
        let initial_scorecard = vec![
            mock_score_entry(
                RoutingProfile::HomeControl,
                "openrouter",
                "davis_1_openrouter",
                "openai/gpt-4o",
                true,
                92.0,
                92,
                95,
                82,
                95,
                60,
            ),
            mock_score_entry(
                RoutingProfile::HomeControl,
                "openrouter",
                "davis_1_openrouter",
                "anthropic/claude-sonnet-4.6",
                true,
                90.0,
                93,
                94,
                75,
                95,
                40,
            ),
            mock_score_entry(
                RoutingProfile::GeneralQa,
                "openrouter",
                "davis_1_openrouter",
                "openai/gpt-4o",
                true,
                90.0,
                89,
                92,
                80,
                95,
                60,
            ),
            mock_score_entry(
                RoutingProfile::Research,
                "openrouter",
                "davis_1_openrouter",
                "openai/gpt-4o",
                true,
                89.0,
                89,
                92,
                78,
                95,
                60,
            ),
            mock_score_entry(
                RoutingProfile::StructuredLookup,
                "openrouter",
                "davis_1_openrouter",
                "openai/gpt-4o",
                true,
                90.0,
                90,
                92,
                82,
                95,
                60,
            ),
        ];
        let initial_plan = build_route_plan(&config, &initial_scorecard).unwrap();
        let initial_home = initial_plan
            .routes
            .iter()
            .find(|route| route.profile == RoutingProfile::HomeControl)
            .unwrap();
        assert_eq!(initial_home.primary.model, "openai/gpt-4o");

        let fluctuated_scorecard = vec![
            mock_score_entry(
                RoutingProfile::HomeControl,
                "openrouter",
                "davis_1_openrouter",
                "openai/gpt-4o",
                true,
                86.0,
                87,
                91,
                70,
                88,
                60,
            ),
            mock_score_entry(
                RoutingProfile::HomeControl,
                "openrouter",
                "davis_1_openrouter",
                "anthropic/claude-sonnet-4.6",
                true,
                92.5,
                94,
                95,
                78,
                93,
                40,
            ),
            mock_score_entry(
                RoutingProfile::GeneralQa,
                "openrouter",
                "davis_1_openrouter",
                "openai/gpt-4o",
                true,
                90.0,
                89,
                92,
                80,
                95,
                60,
            ),
            mock_score_entry(
                RoutingProfile::Research,
                "openrouter",
                "davis_1_openrouter",
                "openai/gpt-4o",
                true,
                89.0,
                89,
                92,
                78,
                95,
                60,
            ),
            mock_score_entry(
                RoutingProfile::StructuredLookup,
                "openrouter",
                "davis_1_openrouter",
                "openai/gpt-4o",
                true,
                90.0,
                90,
                92,
                82,
                95,
                60,
            ),
        ];
        let fluctuated_plan = build_route_plan(&config, &fluctuated_scorecard).unwrap();
        let fluctuated_home = fluctuated_plan
            .routes
            .iter()
            .find(|route| route.profile == RoutingProfile::HomeControl)
            .unwrap();
        assert_eq!(fluctuated_home.primary.provider, "openrouter");
        assert_eq!(fluctuated_home.primary.model, "anthropic/claude-sonnet-4.6");
    }

    #[tokio::test]
    async fn spawn_renders_static_plan_with_current_local_config() {
        let mut config = sample_config();
        config.home_assistant.url = "http://homeassistant.local:8123/api/mcp".to_string();
        config.webhook.secret = "shortcut-shared-secret".to_string();

        let paths = test_runtime_paths("static-plan-render");
        fs::create_dir_all(paths.config_template_path().parent().unwrap()).unwrap();
        fs::write(
            paths.config_template_path(),
            r#"
default_provider = "__DAVIS_DEFAULT_PROVIDER__"
default_model = "__DAVIS_DEFAULT_MODEL__"
__DAVIS_MODEL_PROVIDERS__
__DAVIS_IMESSAGE_CONFIG__
__DAVIS_WEBHOOK_SECRET_CONFIG__
__DAVIS_QUERY_CLASSIFICATION_CONFIG__
__DAVIS_MODEL_ROUTES_CONFIG__
[reliability]
fallback_providers = __DAVIS_FALLBACK_PROVIDERS__
__DAVIS_MODEL_FALLBACKS__
[[mcp.servers]]
name = "homeassistant"
url = "__DAVIS_HA_URL__"
headers = { Authorization = "Bearer __DAVIS_HA_TOKEN__" }
"#,
        )
        .unwrap();

        let _manager = ModelRoutingManager::spawn(paths.clone(), config).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let rendered = fs::read_to_string(paths.runtime_config_path()).unwrap();
        assert!(rendered.contains("/api/mcp"));
        assert!(!rendered.contains("__DAVIS_HA_URL__"));
        assert!(rendered.contains("default_provider = \"davis_1_openrouter\""));
        assert!(rendered.contains("default_model = \"openai/gpt-4o\""));
        assert!(rendered.contains("secret = \"shortcut-shared-secret\""));
    }
}
