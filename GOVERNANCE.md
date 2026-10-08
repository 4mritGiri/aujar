# Governance

- **Maintainers** merge changes, cut releases and enforce the Code of Conduct. Listed in
  `.github/CODEOWNERS`.
- **Contributors** propose changes via pull requests.
- **Decisions** are made by lazy consensus in issues/PRs. Significant or hard-to-reverse
  changes (protocol, plugin model, new dependencies of note) need an
  [ADR](docs/adr/0001-record-architecture-decisions.md) and approval from two maintainers.
- **Becoming a maintainer:** sustained, high-quality contributions plus nomination by an
  existing maintainer and no objection within 7 days.
- **Releases:** semantic versioning. IPC protocol and module API versions are bumped
  independently and documented in the CHANGELOG.
- **License:** Apache-2.0; contributions are accepted under the same license (DCO sign-off).
