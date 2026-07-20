//! OpenAPI generation — the app-agnostic driver TAIL + the TypeScript client
//! generator (`emit_ts`), moved from ziee's `openapi/mod.rs` in Chunk B6.
//!
//! The generator is a **pure function** of the spec (`emit_ts`), and the tail of
//! the generation driver — `finish_api → serialize openapi.json → emit_ts →
//! write types.ts` — is likewise app-agnostic once the OUTPUT PATHS are
//! parameterized (they were hardcoded `../src/api-client/types.ts` in ziee).
//!
//! What stays app-side (in each app's thin driver): loading the app `Config`,
//! setting the process globals (`set_app_data_dir` / `set_caches_config` /
//! `init_repositories`), instantiating + initializing modules, and — for the
//! desktop binary — merging the desktop routes into the server router. Those
//! feed a finished `(ApiRouter, OpenApi)` here, and this function is the shared,
//! byte-identical emit tail both binaries call.

use aide::axum::ApiRouter;
use aide::openapi::OpenApi;
use std::fs;
use std::path::Path;

pub mod emit_ts;

/// Finish the API router, serialize the OpenAPI doc to `<output_dir>/openapi.json`,
/// generate the TypeScript client types from that JSON via [`emit_ts`], and write
/// them to `types_ts_path`.
///
/// Both output locations are **parameterized** (the app supplies them) — the
/// former hardcoded `../src/api-client/types.ts` derivation now lives at the call
/// site, so a second app can emit to its own layout. The generation itself is
/// byte-for-byte identical to the pre-extraction path (same `finish_api`, same
/// `to_string_pretty`, same `generate_types_ts_from_json`).
pub fn finish_and_emit(
    router: ApiRouter,
    mut api_doc: OpenApi,
    output_dir: &Path,
    types_ts_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Finish the API and extract the OpenAPI spec.
    let _router = router.finish_api(&mut api_doc);

    // Serialize to JSON.
    let json = serde_json::to_string_pretty(&api_doc)?;

    // Ensure output directory exists.
    if !output_dir.exists() {
        fs::create_dir_all(output_dir)?;
    }

    // Write openapi.json.
    let openapi_json_path = output_dir.join("openapi.json");
    fs::write(&openapi_json_path, &json)?;
    println!(
        "✓ OpenAPI specification written to: {}",
        openapi_json_path.display()
    );

    // Generate TypeScript types directly (Rust port of the former
    // `ui/openapi/generate-endpoints.ts`) and write to the caller-supplied path.
    let types_ts = emit_ts::generate_types_ts_from_json(&json)?;
    if let Some(parent) = types_ts_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(types_ts_path, &types_ts)?;
    println!("✓ TypeScript types written to: {}", types_ts_path.display());

    // Emit the split-out `permissionDescriptions.ts` sibling (see emit_ts). Kept
    // out of `types.ts` so the label map loads only in the lazy permission
    // picker, not the eager entry chunk.
    let descriptions_ts = emit_ts::generate_permission_descriptions_ts_from_json(&json)?;
    let descriptions_path = types_ts_path.with_file_name("permissionDescriptions.ts");
    fs::write(&descriptions_path, &descriptions_ts)?;
    println!(
        "✓ Permission descriptions written to: {}",
        descriptions_path.display()
    );

    // Emit the split-out `permissions.ts` (the Permissions enum). Kept out of
    // types.ts so value uses inline at build and the enum object rides the lazy
    // permission-picker chunk instead of the entry chunk.
    let permissions_ts = emit_ts::generate_permissions_ts_from_json(&json)?;
    let permissions_path = types_ts_path.with_file_name("permissions.ts");
    fs::write(&permissions_path, &permissions_ts)?;
    println!("✓ Permissions enum written to: {}", permissions_path.display());

    println!("\n✓ OpenAPI generation complete!");
    println!("  - OpenAPI spec: {}", openapi_json_path.display());
    println!("  - TypeScript types: {}", types_ts_path.display());

    Ok(())
}
