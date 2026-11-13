# Maintainers

Remik is maintained in [tinkle-oss/remik](https://github.com/tinkle-oss/remik). This file records known project authority and the process for changing it; it is not a live export of GitHub permissions.

| Account | Role | Responsibilities |
| --- | --- | --- |
| [@oscarlaw](https://github.com/oscarlaw) | Maintainer and organization administrator | Technical direction, review/merge decisions, repository access and settings, release approval, security coordination, and conduct enforcement |

No other human maintainer is listed here. [Releases](docs/releases.md#administrator-setup) use the built-in `GITHUB_TOKEN`, without a separate GitHub App or App credentials. [.github/CODEOWNERS](.github/CODEOWNERS) routes review requests and does not itself grant access or enforce branch protection.

## Decisions and access

Use [issues](https://github.com/tinkle-oss/remik/issues) for usage obstacles and design proposals, and reviewed PRs for changes. The maintainer decides scope after considering compatibility, resource costs, tests, and maintenance burden. Record material decisions in the issue/PR and relevant documentation. Follow [CONTRIBUTING.md](CONTRIBUTING.md) for proposal and regression-test requirements.

Before adding a maintainer or escalating permissions, review their contribution history, judgment on security/compatibility, and willingness to maintain the project. Grant only the permissions needed, require MFA for sensitive access, and update this roster and CODEOWNERS in a reviewed PR. Review and revoke access when responsibilities end; rotate affected secrets. Do not grant organization administration merely to enable code review.

The [PR-only main ruleset](.github/TOOLING.md#remote-main-policy) is already applied, with zero required approvals, no bypass actors, and no required status checks. Maintainers must manually review changes and passing **CI required** results before merging; the check name is not a GitHub-enforced merge gate. Private vulnerability reporting is enabled, as recorded in [SECURITY.md](SECURITY.md).

The remaining release setup is Actions PR permission and package access. Organization policy blocked the repository PR-permission setting with HTTP `409`; the current CLI session lacks `admin:org`, so an organization owner may need to change organization settings. This blocker is not confirmed resolved; follow the [tooling runbook](.github/TOOLING.md#release-setup-and-bootstrap-maintainer-action-required).

## Contact and availability

Public bugs and proposals belong in GitHub issues. Follow [SECURITY.md](SECURITY.md#report-a-vulnerability) for vulnerabilities and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md#reporting-and-enforcement) for conduct concerns. Support is best effort; there is no on-call rotation, guaranteed issue turnaround, or 24/7 service.

Reviewers should disclose conflicts of interest and step aside where feasible. There is currently one named maintainer, so an independent internal appeal or backup security contact has not been designated. Use [GitHub's abuse-reporting process](https://docs.github.com/en/site-policy/acceptable-use-policies/github-community-guidelines#reporting-abuse) for platform-policy violations when an internal route is unsuitable.
