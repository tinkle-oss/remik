# Security policy

Remik `0.1.0-rc.1` is experimental, memory-only software for trusted networks. It has no TLS, ACLs, persistence, tenant isolation, or availability SLA. Authentication does not encrypt passwords or traffic, and any authorized client can read, overwrite, delete, or flush all data.

## Report a vulnerability

Do not post exploit details, credentials, or sensitive data in public issues or pull requests. Use [GitHub private vulnerability reporting](https://github.com/tinkle-oss/remik/security/advisories/new), addressed to the repository maintainers. Private reporting is enabled for this repository.

If GitHub does not offer “Report a vulnerability,” contact [@oscarlaw](https://github.com/oscarlaw) through a contact method explicitly published on that profile, if one is available. Otherwise, open a nonsensitive issue asking the maintainer to enable private reporting or arrange a private channel, without disclosing the vulnerability. No project security email or private messaging service is currently designated. Do not assume a public issue is confidential.

In the private report, include:

- Affected version/commit, OS/architecture, and redacted configuration.
- Impact, required access, and a minimal reproduction or proof of concept.
- Expected versus observed behavior, including resource or authentication boundaries crossed.
- Whether the issue is already public, and your preferred attribution/disclosure contact.

Maintainers aim to acknowledge reports within **7 calendar days, best effort**, then agree on investigation updates and a coordinated disclosure date with the reporter. This is not a guaranteed response or fix deadline, and there is no 24/7 security support or bug bounty commitment. If no response arrives, follow up in the same private channel; a public reminder should omit technical details.

For a confirmed issue, the process is to assess impact, develop a fix and regression test privately where possible, prepare a new release and mitigation guidance, and publish a GitHub advisory with affected/fixed versions and reporter credit if requested. Coordinate publication with reporters and affected upstreams; do not promise a release date before a fix is validated.

## Support scope

| Version | Security maintenance policy |
| --- | --- |
| `main` | Best-effort development fixes; not a supported release artifact |
| Latest published `0.1.0-rc.N` | Best-effort fixes through a subsequent candidate; no fixed support duration |
| Superseded candidates | No routine backports; upgrade to the latest candidate |
| Stable releases | No stable support line is established |

The initial source version is `0.1.0-rc.1`; its presence in the manifest is not proof of publication. Prereleases may change their wire, CLI, or Rust API contracts. See [releases](docs/releases.md) for distribution status and policy.

## Deployment assumptions

Keep the native loopback default or restrict access with trusted network controls. Docker publication examples bind the host port to `127.0.0.1`; the container itself listens on all interfaces and should not share a network with untrusted peers. There is no Redis-style protected-mode safety net when you change the bind address.

Use a runtime password file with restrictive permissions; never bake it into an image, commit it, or place its contents in command arguments. Keep `RUST_LOG` filters and captured diagnostics free of secrets. The implementation avoids reflecting protocol input in errors, but that does not make arbitrary application/client logs safe to publish.

Resource limits reduce exposure without guaranteeing resistance to denial of service. The dataset budget excludes read/write buffers, retained table capacity, transient work, and process/runtime memory; see [resource planning](docs/operations.md#resource-planning). A container memory limit can terminate the process. Restarts, including password rotation and security upgrades, lose the entire dataset.

## Dependency and release security

[CI](.github/workflows/ci.yml) runs Cargo advisory/license/source/bans checks and scans fixable HIGH/CRITICAL image vulnerabilities. Both root and fuzz dependency graphs are checked; scanner databases refresh, so results can change without a code change. Violations must be resolved or receive a narrow, documented maintainer review before release; blanket suppressions are not acceptable. These checks are not a comprehensive security audit or a SAST certification.

PR jobs use read-only repository permissions and no release secrets. Publication is separated into trusted-main jobs against an exact release SHA. Release images request BuildKit SBOM and provenance attestations; native archives have checksums and license notices, not an implemented signature-verification chain. See [artifact verification](docs/releases.md#verify-published-artifacts) and the [tooling runbook](.github/TOOLING.md).

Repository administrators must require MFA for sensitive accounts, review permissions before granting write/admin access, protect `main`, and enable private reporting. Store release App credentials only in the designated Actions secret, restrict access, and revoke/rotate them on compromise or access changes. These are administrator responsibilities, not settings this documentation has verified or applied.

The [OpenSSF OSPS Baseline v2026.08.28](https://baseline.openssf.org/versions/2026-08-28.html) informs the project checklist. Remik does not claim baseline compliance or certification; private reporting, branch protection, credential setup, and release evidence require administrator validation.
