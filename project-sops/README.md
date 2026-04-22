# project-sops/

User-defined ZeroClaw **SOPs** (Standard Operating Procedures) live here. Each
SOP is a runbook ZeroClaw can execute step-by-step under your supervision —
not a natural-language router, not a skill.

Davis itself ships **zero SOPs by default**; this directory exists so that
`daviszeroclaw sops sync` / `sops check` have a well-known place to scan.

## Adding a SOP

Create a directory with at minimum a `SOP.toml`:

```
project-sops/
└── my-sop/
    ├── SOP.toml       # required: metadata + triggers + steps
    └── SOP.md         # optional: human-readable runbook
```

See the ZeroClaw SOP schema (`zeroclaw sop --help`) for the `SOP.toml` shape.

After adding or editing, apply:

```
daviszeroclaw sops sync       # copies project-sops/ into .runtime/davis/sops/
daviszeroclaw sops check      # validates each SOP through ZeroClaw
```

## Skills vs. SOPs

Both live near each other but do different things:

| Kind  | Directory        | Purpose                                                  |
|-------|------------------|----------------------------------------------------------|
| Skill | `project-skills/`| Short prompt-level guidance for the agent                |
| SOP   | `project-sops/`  | Deterministic multi-step runbook executed by ZeroClaw    |

If you're not sure which you want, it's almost always a skill.
