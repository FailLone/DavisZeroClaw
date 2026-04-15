# HA Reconciliation API

## Replacement Candidates

`GET http://127.0.0.1:3010/advisor/replacement-candidates`

Important fields:

- `candidate_count`
- `high_confidence_count`
- `needs_review_count`
- `candidates[*].unavailable_name`
- `candidates[*].replacement_name`
- `candidates[*].domain`
- `candidates[*].score`
- `candidates[*].confidence`
- `candidates[*].reasons`
- `candidates[*].suggested_actions`

Interpretation notes:

- `high_confidence` still means "review before changing aliases/groups", not "auto-migrate now".
- If a candidate is same-name and same-domain but lacks area evidence, treat it as a likely duplicate exposure until proven otherwise.
