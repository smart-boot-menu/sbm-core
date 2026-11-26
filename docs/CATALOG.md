# Catalog plans

`sbm-catalog` publishes `OsCatalogEntry` documents and builds an `OsInstallPlan` whose `stable_id` is the literal `unbound`. `inspect_catalog_plan` in `sbm-runtime` is the consumer.

It accepts the plan only when all of these hold:

- both schema versions are `1.0`
- `catalog_entry_id` and `catalog_release_version` match the entry
- `target_arch` is one of the release architectures
- every `artifact_verification` item matches a catalog artifact filename, size, and SHA-256

An accepted unbound plan returns `execution_enabled: false` and says the plan is not executable. `EXECUTION_ENABLED_IN_0_1` remains false for every plan. A different hash returns `SignatureInvalid`. Storage operations while `stable_id` is `unbound` return `DiskIdentityMismatch` and are not executed.

This function does not download an image, does not open a disk, and does not replace `unbound` with a live `DiskIdentity`. Compatibility sidecars and declarative recipes stay in `sbm-catalog`. They are not required to inspect the canonical plan.
