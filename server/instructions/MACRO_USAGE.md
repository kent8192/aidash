# Macro Usage

Prefer Reinhardt macros for the surface they own. They carry route metadata,
registration, validation, and API contract behavior that hand-written
equivalents can miss.

## `#[routes]`

Keep project-level route composition in `src/config/urls.rs`. Mount each app's
`UnifiedRouter` aggregate with `mount_unified`; app-local `url_patterns()`
functions configure HTTP endpoints through `.server(...)`.

```rust,ignore
#[routes]
pub fn routes() -> UnifiedRouter {
    UnifiedRouter::new().mount_unified(
        "/",
        crate::apps::users::urls::url_patterns(),
    )
}
```

The prefix should be a literal path. Dynamic parameters belong in the app's
endpoint route, not in the project mount prefix.

Keep a single `#[routes]` registration on the project root. The
`#[url_patterns]` attribute removes native HTTP expressions from shared
native/WASM builders when `cfg(server)` is inactive; Aidash's native backend
does not need that target-dependent transformation.

## Endpoint macros

- Use `#[get]`, `#[post]`, `#[put]`, and `#[delete]` for HTTP method metadata.
- Give every public endpoint a stable `name` for route inspection and reverse
  lookup.
- Use `pre_validate = true` when every extractor implements `Validate`.
  If a handler mixes `Json<T>` with a primitive `Path<T>`, validate the JSON
  value explicitly instead of applying the option to incompatible extractors.
- Keep handlers in the app's `views` module and expose them through the app's
  `urls` aggregate.

```rust,ignore
#[post("/users/", name = "users-create", pre_validate = true)]
pub async fn create(
    Json(payload): Json<CreateUser>,
) -> ViewResult<Response> {
    let _ = payload;
    Ok(Response::new(StatusCode::CREATED))
}
```

For a literal health endpoint, use the same metadata-bearing form:

```rust,ignore
#[get("/health/", name = "health")]
pub async fn health() -> ViewResult<Response> {
    Ok(Response::new(StatusCode::OK).with_body("ok".to_string()))
}
```

## `#[model(...)]` and serializers

- `#[model(...)]` applies the model derive; do not add a redundant
  `#[derive(Model)]`.
- Prefer `Model::build()` and its generated typestate builder over struct
  literals when creating records.
- Keep request serializers separate from response DTOs when the API exposes
  different writable and readable fields.
- Mark sensitive model fields with the appropriate skip attribute before
  generating cross-layer info payloads.

```rust,ignore
#[model(app_label = "users", table_name = "users")]
#[derive(Serialize, Deserialize)]
pub struct User {
    #[field(primary_key = true)]
    pub id: i64,
    #[field(max_length = 255)]
    pub email: String,
}

let user = User::build().email("user@example.com").finish();
```

## Application and authorization macros

Register each generated app with `#[app_config]`; `startapp` keeps the module
and installed-app registries synchronized. Use `Depends<T>` for app services,
`CurrentUser<U>` for authenticated users, and `guard!()` for declarative
endpoint authorization.

```rust,ignore
#[app_config(name = "users", label = "users")]
pub struct UsersConfig;

#[get("/admin/", name = "admin", guards = guard!(IsStaff))]
pub async fn admin_only() -> ViewResult<Response> {
    Ok(Response::new(StatusCode::OK))
}
```

Use framework serializers, route helpers, and typed builders instead of
parallel local infrastructure.
