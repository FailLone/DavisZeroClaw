# HA Advisor API

## Configuration Report

`GET http://127.0.0.1:3010/advisor/config-report`

Important fields:

- `counts`
- `recent_failures`
- `findings.duplicate_friendly_names`
- `findings.cross_domain_conflicts`
- `findings.missing_room_semantic`
- `suggestions.entity_aliases`
- `suggestions.groups`
- `suggestions.assist_entities`
- `suggestions.custom_sentences`
- `suggestions.migration_suggestions`
- `advanced_opportunities`

## Failure Summary

`GET http://127.0.0.1:3010/advisor/failure-summary`

Important fields:

- `failure_count`
- `counts_by_reason`
- `top_failed_queries`
- `events`
- `suggestion_due`

Interpretation notes:

- Treat `counts_by_reason.resolution_ambiguous` as a configuration-quality signal rather than a transient transport failure.
- When `resolution_ambiguous` is present, correlate it with duplicate names, weak aliases, or missing room/group semantics in the config report.
- `suggestions.migration_suggestions` contains read-only proposals for alias migration or group-member replacement. Review the `snippet` before applying anything to `control_aliases.json`.
