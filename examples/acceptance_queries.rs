//! SQL for the Python acceptance harness, generated with the same query builder
//! as the application. This helper does not connect to a database.
use sea_orm::sea_query::{Alias, Asterisk, Expr, PostgresQueryBuilder, Query};
fn main() {
    let pending = Query::select()
        .expr(Expr::col(Asterisk).count())
        .from(Alias::new("events"))
        .and_where(Expr::col(Alias::new("published_at")).is_null())
        .to_string(PostgresQueryBuilder);
    let inbox = Query::select()
        .expr(Expr::col(Asterisk).count())
        .from(Alias::new("inbox"))
        .to_string(PostgresQueryBuilder);
    println!(
        "{}",
        serde_json::json!({"pending_events":pending,"inbox":inbox})
    );
}
