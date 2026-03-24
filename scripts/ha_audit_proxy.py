#!/usr/bin/env python3
import json
import os
from collections import Counter
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError, URLError
from urllib.parse import parse_qs, quote, urlparse
from urllib.request import Request, urlopen


LISTEN_HOST = "127.0.0.1"
LISTEN_PORT = 3010
DEFAULT_WINDOW_MINUTES = 60

ISSUE_METADATA = {
    "missing_credentials": {
        "category": "configuration",
        "recommended_actions": [
            "Set DAVIS_HA_URL in .env.local",
            "Set DAVIS_HA_TOKEN in .env.local",
        ],
        "missing_requirements": ["ha_url", "ha_token"],
    },
    "ha_unreachable": {
        "category": "connectivity",
        "recommended_actions": [
            "Verify DAVIS_HA_URL is reachable",
            "Check network, DNS, reverse proxy, and TLS configuration",
        ],
        "missing_requirements": [],
    },
    "ha_auth_failed": {
        "category": "authorization",
        "recommended_actions": [
            "Verify the current Long-Lived Access Token",
            "Regenerate DAVIS_HA_TOKEN only if the current token cannot access HA REST endpoints",
        ],
        "missing_requirements": [],
    },
    "recorder_not_enabled": {
        "category": "configuration",
        "recommended_actions": [
            "Enable recorder in Home Assistant",
            "Confirm the target entity is not excluded from recorder",
            "Confirm history/logbook retention covers the requested window",
        ],
        "missing_requirements": ["recorder", "history", "logbook"],
    },
    "entity_not_found": {
        "category": "resolution",
        "recommended_actions": [
            "Use a more specific entity_id",
            "Try the entity's friendly name",
            "Inspect Home Assistant states to confirm the real entity_id",
        ],
        "missing_requirements": [],
    },
    "entity_ambiguous": {
        "category": "resolution",
        "recommended_actions": [
            "Use the full entity_id instead of a shorthand",
            "Choose one of the suggested candidate entities",
        ],
        "missing_requirements": [],
    },
    "bad_request": {
        "category": "request",
        "recommended_actions": [
            "Provide entity_id",
            "Use a valid time range where start is earlier than end",
        ],
        "missing_requirements": ["entity_id"],
    },
}


def utc_now():
    return datetime.now(timezone.utc)


def isoformat(dt: datetime) -> str:
    return dt.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def normalize_text(text: str) -> str:
    return "".join(ch.lower() for ch in text.strip() if ch not in {" ", "_", "-", "."})


def derive_ha_origin(ha_url: str) -> str:
    parsed = urlparse(ha_url)
    if not parsed.scheme or not parsed.netloc:
        raise ValueError("DAVIS_HA_URL 不是合法 URL")
    return f"{parsed.scheme}://{parsed.netloc}"


def build_ha_request(path: str) -> Request:
    ha_url = os.environ.get("DAVIS_HA_URL", "")
    ha_token = os.environ.get("DAVIS_HA_TOKEN", "")
    if not ha_url or not ha_token:
        raise RuntimeError("缺少 DAVIS_HA_URL 或 DAVIS_HA_TOKEN")

    origin = derive_ha_origin(ha_url)
    return Request(
        f"{origin}{path}",
        headers={
            "Authorization": f"Bearer {ha_token}",
            "Content-Type": "application/json",
            # Cloudflare on this HA origin rejects urllib's default Python user-agent.
            "User-Agent": "curl/8.7.1",
            "Accept": "application/json",
        },
        method="GET",
    )


def fetch_json(path: str):
    request = build_ha_request(path)
    with urlopen(request, timeout=20) as response:
        payload = response.read()
    return json.loads(payload.decode("utf-8"))


def normalize_window(query):
    if "start" in query:
        start = datetime.fromisoformat(query["start"][0].replace("Z", "+00:00"))
    else:
        minutes = int(query.get("window_minutes", [str(DEFAULT_WINDOW_MINUTES)])[0])
        start = utc_now() - timedelta(minutes=minutes)

    if "end" in query:
        end = datetime.fromisoformat(query["end"][0].replace("Z", "+00:00"))
    else:
        end = utc_now()

    if start >= end:
        raise ValueError("start 必须早于 end")
    return start, end


def build_issue(issue_type: str, query_entity: str, details=None):
    details = details or {}
    metadata = ISSUE_METADATA.get(
        issue_type,
        {
            "category": "unknown",
            "recommended_actions": ["Inspect the HA audit proxy response details"],
            "missing_requirements": [],
        },
    )
    return {
        "issue_type": issue_type,
        "issue_category": metadata["category"],
        "query_entity": query_entity,
        "recommended_actions": metadata["recommended_actions"],
        "missing_requirements": metadata["missing_requirements"],
        "suggestions": (details.get("suggestions") or [])[:5],
    }


def fetch_all_states():
    states = fetch_json("/api/states")
    return states if isinstance(states, list) else []


def resolve_entity(query_entity: str, all_states):
    raw = query_entity.strip()
    raw_norm = normalize_text(raw)
    if not raw_norm:
        return {"status": "not_found", "suggestions": []}

    scored = []
    for state in all_states:
        entity_id = state.get("entity_id", "")
        friendly_name = state.get("attributes", {}).get("friendly_name", "")
        suffix = entity_id.split(".", 1)[1] if "." in entity_id else entity_id

        if raw == entity_id:
            score = 100
            matched_by = "exact_entity_id"
        elif raw == suffix:
            score = 95
            matched_by = "exact_suffix"
        elif raw == friendly_name:
            score = 90
            matched_by = "exact_friendly_name"
        else:
            entity_norm = normalize_text(entity_id)
            suffix_norm = normalize_text(suffix)
            name_norm = normalize_text(friendly_name)

            if raw_norm == suffix_norm:
                score = 85
                matched_by = "normalized_suffix"
            elif raw_norm == name_norm:
                score = 80
                matched_by = "normalized_friendly_name"
            elif raw_norm == entity_norm:
                score = 75
                matched_by = "normalized_entity_id"
            elif raw_norm in suffix_norm or raw_norm in name_norm:
                score = 40
                matched_by = "partial_match"
            else:
                continue

        scored.append((score, entity_id, state, matched_by))

    if not scored:
        suggestions = []
        for state in all_states:
            entity_id = state.get("entity_id", "")
            friendly_name = state.get("attributes", {}).get("friendly_name", "")
            haystack = " ".join([entity_id, friendly_name]).lower()
            if raw.lower() in haystack:
                suggestions.append(entity_id)
        return {"status": "not_found", "suggestions": suggestions[:5]}

    scored.sort(key=lambda item: (-item[0], item[1]))
    best_score = scored[0][0]
    best = [item for item in scored if item[0] == best_score]

    if len(best) > 1 and best_score < 95:
        return {
            "status": "ambiguous",
            "suggestions": [item[1] for item in best[:5]],
        }

    return {
        "status": "ok",
        "entity_id": best[0][1],
        "state": best[0][2],
        "matched_by": best[0][3],
        "suggestions": [item[1] for item in scored[:5]],
    }


def related_entity_ids(primary_entity_id: str, all_states):
    all_entity_ids = {state.get("entity_id", "") for state in all_states}
    related = []

    if primary_entity_id.startswith("binary_sensor.") and primary_entity_id.endswith("_on_off"):
        stem = primary_entity_id[len("binary_sensor.") : -len("_on_off")]
        climate_id = f"climate.{stem}"
        if climate_id in all_entity_ids:
            related.append(climate_id)

    if primary_entity_id.startswith("climate."):
        stem = primary_entity_id[len("climate.") :]
        binary_id = f"binary_sensor.{stem}_on_off"
        if binary_id in all_entity_ids:
            related.append(binary_id)

    return related


def resolve_entity_payload(query_entity: str):
    try:
        all_states = fetch_all_states()
    except RuntimeError:
        return {
            "status": "config_issue",
            "issue": build_issue("missing_credentials", query_entity),
        }
    except HTTPError as exc:
        issue_type = "ha_auth_failed" if exc.code in (401, 403) else "ha_unreachable"
        return {
            "status": "config_issue",
            "issue": build_issue(issue_type, query_entity),
        }
    except (URLError, ValueError):
        return {
            "status": "config_issue",
            "issue": build_issue("ha_unreachable", query_entity),
        }

    resolution = resolve_entity(query_entity, all_states)
    if resolution["status"] == "not_found":
        return {
            "status": "not_found",
            "query_entity": query_entity,
            "suggestions": resolution.get("suggestions", []),
        }
    if resolution["status"] == "ambiguous":
        return {
            "status": "ambiguous",
            "query_entity": query_entity,
            "suggestions": resolution.get("suggestions", []),
        }

    state = resolution["state"]
    entity_id = resolution["entity_id"]
    return {
        "status": "ok",
        "query_entity": query_entity,
        "resolved_entity_id": entity_id,
        "matched_by": resolution.get("matched_by"),
        "friendly_name": state.get("attributes", {}).get("friendly_name"),
        "domain": entity_id.split(".", 1)[0] if "." in entity_id else None,
        "current_state": state.get("state"),
        "related_entity_ids": related_entity_ids(entity_id, all_states),
        "suggestions": resolution.get("suggestions", []),
    }


def fetch_history_rows(entity_id: str, start: datetime, end: datetime):
    rows = fetch_json(
        f"/api/history/period/{quote(isoformat(start), safe=':TZ-')}?end_time={quote(isoformat(end), safe=':TZ-')}&filter_entity_id={quote(entity_id, safe='._')}"
    )
    return rows[0] if isinstance(rows, list) and rows else []


def fetch_logbook_rows(entity_id: str, friendly_name: str, start: datetime, end: datetime):
    direct = fetch_json(
        f"/api/logbook/{quote(isoformat(start), safe=':TZ-')}?end_time={quote(isoformat(end), safe=':TZ-')}&entity={quote(entity_id, safe='._')}"
    )
    direct = direct if isinstance(direct, list) else []

    global_rows = fetch_json(
        f"/api/logbook/{quote(isoformat(start), safe=':TZ-')}?end_time={quote(isoformat(end), safe=':TZ-')}"
    )
    global_rows = global_rows if isinstance(global_rows, list) else []

    merged = {}
    for row in direct + global_rows:
        message = row.get("message", "") or ""
        name = row.get("name", "") or ""
        if (
            row.get("entity_id") == entity_id
            or row.get("context_entity_id") == entity_id
            or (friendly_name and friendly_name in message)
            or (friendly_name and name == "HomeKit" and friendly_name in message)
        ):
            key = (
                row.get("when"),
                row.get("entity_id"),
                row.get("name"),
                row.get("message"),
                row.get("context_entity_id"),
            )
            merged[key] = row

    return sorted(merged.values(), key=lambda row: row.get("when") or "")


def collect_actor(logbook_rows):
    for row in logbook_rows:
        context_user_id = row.get("context_user_id")
        if context_user_id:
            return {
                "type": "user",
                "id": context_user_id,
                "name": row.get("name"),
            }
    return {
        "type": "unknown",
        "id": None,
        "name": None,
    }


def collect_source(history_rows, logbook_rows):
    source_values = [
        row.get("attributes", {}).get("source", "")
        for row in history_rows
        if row.get("attributes", {}).get("source")
    ]
    source_counter = Counter(source_values)
    primary_source = source_counter.most_common(1)[0][0] if source_counter else None

    observations = []
    for row in logbook_rows:
        if row.get("name") == "HomeKit" and row.get("message"):
            observations.append(
                {
                    "type": "integration_command",
                    "integration": "HomeKit",
                    "time": row.get("when"),
                    "message": row.get("message"),
                }
            )

    if primary_source or observations:
        return {
            "type": "integration_signal",
            "id": primary_source,
            "observations": observations,
        }

    return {
        "type": "unknown",
        "id": None,
        "observations": [],
    }


def build_timeline(entity_id: str, history_rows, logbook_rows):
    timeline = []

    for row in logbook_rows:
        timeline.append(
            {
                "time": row.get("when"),
                "entity_id": entity_id,
                "source": "logbook",
                "name": row.get("name"),
                "message": row.get("message"),
                "context_entity_id": row.get("context_entity_id"),
                "context_state": row.get("context_state"),
            }
        )

    for row in history_rows:
        timeline.append(
            {
                "time": row.get("last_changed"),
                "entity_id": entity_id,
                "source": "history",
                "state": row.get("state"),
                "friendly_name": row.get("attributes", {}).get("friendly_name"),
                "upstream_source": row.get("attributes", {}).get("source"),
            }
        )

    timeline.sort(key=lambda item: item.get("time") or "")
    return timeline


def confidence_for(actor, source):
    if actor["type"] == "user":
        return "high"
    if source.get("id") or source.get("observations"):
        return "medium"
    return "low"


def audit_entity(query_entity: str, start: datetime, end: datetime):
    try:
        config = fetch_json("/api/config")
        all_states = fetch_all_states()
    except RuntimeError:
        return {
            "result_type": "config_issue",
            "issue": build_issue("missing_credentials", query_entity),
        }
    except HTTPError as exc:
        if exc.code in (401, 403):
            return {
                "result_type": "config_issue",
                "issue": build_issue("ha_auth_failed", query_entity),
            }
        return {
            "result_type": "config_issue",
            "issue": build_issue("ha_unreachable", query_entity),
        }
    except (URLError, ValueError):
        return {
            "result_type": "config_issue",
            "issue": build_issue("ha_unreachable", query_entity),
        }

    components = set(config.get("components", []))
    if "recorder" not in components or "history" not in components or "logbook" not in components:
        return {
            "result_type": "config_issue",
            "issue": build_issue("recorder_not_enabled", query_entity),
        }

    resolution = resolve_entity(query_entity, all_states)
    if resolution["status"] == "not_found":
        return {
            "result_type": "config_issue",
            "issue": build_issue(
                "entity_not_found",
                query_entity,
                {"suggestions": resolution.get("suggestions", [])},
            ),
        }
    if resolution["status"] == "ambiguous":
        return {
            "result_type": "config_issue",
            "issue": build_issue(
                "entity_ambiguous",
                query_entity,
                {"suggestions": resolution.get("suggestions", [])},
            ),
        }

    primary_entity_id = resolution["entity_id"]
    primary_state = resolution["state"]
    related_ids = related_entity_ids(primary_entity_id, all_states)
    audit_ids = [primary_entity_id] + [entity_id for entity_id in related_ids if entity_id != primary_entity_id]

    try:
        entity_audits = []
        all_history_rows = []
        all_logbook_rows = []
        primary_history_rows = []

        for entity_id in audit_ids:
            state = next((item for item in all_states if item.get("entity_id") == entity_id), {})
            friendly_name = state.get("attributes", {}).get("friendly_name", "")
            history_rows = fetch_history_rows(entity_id, start, end)
            logbook_rows = fetch_logbook_rows(entity_id, friendly_name, start, end)
            timeline = build_timeline(entity_id, history_rows, logbook_rows)

            if entity_id == primary_entity_id:
                primary_history_rows = history_rows

            entity_audits.append(
                {
                    "entity_id": entity_id,
                    "friendly_name": friendly_name,
                    "current_state": state.get("state"),
                    "history_count": len(history_rows),
                    "logbook_count": len(logbook_rows),
                    "timeline": timeline,
                }
            )
            all_history_rows.extend(history_rows)
            all_logbook_rows.extend(logbook_rows)
    except HTTPError as exc:
        if exc.code in (401, 403):
            return {
                "result_type": "config_issue",
                "issue": build_issue("ha_auth_failed", query_entity),
            }
        return {
            "result_type": "config_issue",
            "issue": build_issue("ha_unreachable", query_entity),
        }
    except (URLError, ValueError):
        return {
            "result_type": "config_issue",
            "issue": build_issue("ha_unreachable", query_entity),
        }

    if not all_history_rows and not all_logbook_rows:
        return {
            "result_type": "no_evidence",
            "query_entity": query_entity,
            "resolved_entity_id": primary_entity_id,
            "related_entity_ids": related_ids,
            "window_start": isoformat(start),
            "window_end": isoformat(end),
            "current_state": primary_state.get("state"),
            "queried_sources": ["logbook", "history"],
            "missing_evidence_types": ["state_changes", "logbook_entries", "actor_context"],
            "possible_reasons": [
                "no_matching_activity_in_window",
                "recorder_gap_or_exclusion",
                "history_purged",
            ],
            "confidence": "low",
        }

    actor = collect_actor(all_logbook_rows)
    source = collect_source(all_history_rows, all_logbook_rows)
    confidence = confidence_for(actor, source)
    primary_transition_count = max(len(primary_history_rows) - 1, 0)

    return {
        "result_type": "evidence",
        "query_entity": query_entity,
        "resolved_entity_id": primary_entity_id,
        "matched_by": resolution.get("matched_by"),
        "related_entity_ids": related_ids,
        "window_start": isoformat(start),
        "window_end": isoformat(end),
        "current_state": primary_state.get("state"),
        "actor": actor,
        "source": source,
        "confidence": confidence,
        "findings": {
            "primary_transition_count": primary_transition_count,
            "actor_identified": actor["type"] != "unknown",
            "upstream_source_identified": bool(source.get("id")),
            "integration_observation_count": len(source.get("observations", [])),
            "related_entity_count": len(related_ids),
        },
        "counts": {
            "entities": len(entity_audits),
            "history": len(all_history_rows),
            "logbook": len(all_logbook_rows),
        },
        "entities": entity_audits,
    }


class AuditHandler(BaseHTTPRequestHandler):
    server_version = "DavisHAAuditProxy/0.2"

    def _write_json(self, status: int, payload):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        return

    def do_GET(self):
        parsed = urlparse(self.path)

        if parsed.path == "/health":
            self._write_json(200, {"status": "ok", "service": "ha_audit_proxy"})
            return

        if parsed.path == "/resolve-entity":
            query = parse_qs(parsed.query)
            query_entity = query.get("entity_id", [""])[0].strip()
            if not query_entity:
                self._write_json(
                    400,
                    {
                        "status": "config_issue",
                        "issue": build_issue("bad_request", query_entity),
                    },
                )
                return
            self._write_json(200, resolve_entity_payload(query_entity))
            return

        if parsed.path != "/audit":
            self._write_json(404, {"error": "not_found"})
            return

        query = parse_qs(parsed.query)
        query_entity = query.get("entity_id", [""])[0].strip()
        if not query_entity:
            self._write_json(
                400,
                {
                    "result_type": "config_issue",
                    "issue": build_issue("bad_request", query_entity),
                },
            )
            return

        try:
            start, end = normalize_window(query)
        except (TypeError, ValueError):
            self._write_json(
                400,
                {
                    "result_type": "config_issue",
                    "issue": build_issue("bad_request", query_entity),
                },
            )
            return

        result = audit_entity(query_entity, start, end)
        self._write_json(200, result)


def main():
    server = ThreadingHTTPServer((LISTEN_HOST, LISTEN_PORT), AuditHandler)
    server.serve_forever()


if __name__ == "__main__":
    main()
