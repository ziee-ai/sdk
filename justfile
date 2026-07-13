# ziee SDK justfile (scaffold stub)
# Recipes will grow as the extraction proceeds. See SDK_EXTRACTION_PLAN.md.

# Compile the Rust workspace and run the JS package tests.
check:
    cargo check --workspace
    npm test
