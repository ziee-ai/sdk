# ziee SDK

ziee SDK — the extracted, app-agnostic platform layer (server crates, framework
runtime, kit, and desktop harness) shared across ziee apps. See
`SDK_EXTRACTION_PLAN.md` for the full layout and migration model.

## Auth settings: the refresh cookie name

The httpOnly refresh-token cookie's name is the `auth.refresh_cookie_name`
setting (`ziee_core::AuthConfig`). It defaults to `ziee_refresh`, so an app that
does not write the key is unchanged; another app picks its own, e.g.

```yaml
auth:
  refresh_cookie_name: mangwa_refresh
```

Every code path that sets, reads or clears the cookie uses the configured name
(`ziee_auth::auth::refresh_cookie_name()` is the single read; app code that needs
the name, such as an SSR gate that must not render for a signed-in browser,
calls it rather than keeping a second literal). Allowed characters:
`[A-Za-z0-9_-]`. Changing it on a live deployment signs every browser out once.
