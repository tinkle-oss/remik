## Reason and scope

Describe the problem, link the issue/proposal, and explain the smallest intended change. Major changes need a proposal first. Do not disclose vulnerabilities here; follow SECURITY.md.

## Observable behavior

Describe wire/CLI/API changes, supported and rejected options, failure behavior, and any compatibility, resource, security, or dependency/license impact.

## Validation

List the exact checks run and their results. Mark unrun checks and explain why. For a bug fix, identify the regression test that fails before the fix. Performance claims need raw benchmark evidence and environment metadata.

## Review checklist

- [ ] The change is scoped, and any required major-change proposal is linked.
- [ ] Regression, boundary, and error-side-effect tests are added or updated where relevant.
- [ ] Documentation matches the implemented behavior; unsupported options are not silently accepted.
- [ ] The final commit/squash title follows Conventional Commits for Release Please.
- [ ] I am authorized to submit this work under Apache-2.0; provenance and authorship are accurate, with no copied Redis source.
- [ ] No secrets, private data, generated executables, or unreviewed license/advisory suppressions are included.
