# Dependency Security Audit

**Result:** WARNINGS — review required; this is not a clean audit

## Audit context

| Field | Value |
|---|---|
| Mode | change |
| Completed | 2026\-08\-23T17:32:14\.051661Z |
| Project revision | eee4c6493ef4177bcf43fea8ffa63900194b85b1 |
| Inventory fingerprint | 58c240bbac1925b6b04dc6080602803104c6a8165d63d362eeba34aca668df57 |
| Inventory completeness | incomplete |
| Stable exit code | 0 |

## Report links

- [Machine-readable JSON](latest.json)

## Source availability

| Source | State | Provenance | Diagnostic |
|---|---|---|---|
| cargo\-audit | not\_applicable | not recorded | ecosystem not present |
| govulncheck | not\_applicable | not recorded | ecosystem not present |
| kev | ok | [source](https://www.cisa.gov/sites/default/files/feeds/known_exploited_vulnerabilities.json) | — |
| npm\-audit | not\_applicable | not recorded | ecosystem not present |
| osv | ok | [source](https://api.osv.dev/v1) | — |
| pip\-audit | unavailable | not recorded | native audit executable not found |

## Inventory

Resolved packages: **10**.

Incomplete evidence:
- pip inspect declares requirements but does not provide resolved dependency edges

## Blocking findings (0)

None.

## Warnings (0)

None.

## Excluded findings (0)

None.

## Unclassified findings (0)

None.

## Unmatched decisions (0)

None.

## Remediation and acceptance

No remediation or risk acceptance is recorded because there are no actionable findings.
