# ziee SDK justfile (scaffold stub)
# Recipes will grow as the extraction proceeds. See SDK_EXTRACTION_PLAN.md.

# Compile the Rust workspace and run the JS package tests.
#
# `cargo check --workspace` resolves each member with its DEFAULT features, and
# no member of this workspace enables `ziee-auth/module` — so the turnkey
# `AuthModule` (the whole `module.rs`, its tests, and the auth-sync-wiring
# declaration it enforces) was compiled by nothing here. The only thing exercising
# it was a consuming app's build, i.e. after the push. The extra line below is
# not belt-and-braces: it is the only gate that sees that file at all.
#
# `npm test` at this root has never run: there is no `test` script here, so the
# step exited 1 ("Missing script") and took `just check` with it. The workspaces
# form runs each package's own suite (config / framework / gallery / kit today;
# `--if-present` skips the three that ship none) — which is what the line was
# always meant to be.
check:
    cargo check --workspace
    cargo test -p ziee-auth --features module
    npm test --workspaces --if-present
