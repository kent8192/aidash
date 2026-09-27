use super::{
	ChannelHistoryQuery, ChannelMessage, ChannelMessagePage, access::Lease, attachments, threads,
};
use crate::{Error, Result, domain::Message};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{
	Alias, Asterisk, Condition, Expr, JoinType, Order, PostgresQueryBuilder, Query,
};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct Row {
	#[sqlx(flatten)]
	message: Message,
	reply_thread_id: Option<Uuid>,
	root_thread_id: Option<Uuid>,
}

pub(crate) async fn page(
	lease: &mut Lease,
	workspace: Uuid,
	input: ChannelHistoryQuery,
) -> Result<ChannelMessagePage> {
	if !(1..=100).contains(&input.limit) {
		return Err(Error::Invalid(
			"history limit must be between 1 and 100".into(),
		));
	}
	let thread = match input.thread_id {
		Some(id) => Some(threads::get(lease, workspace, id).await?),
		None => None,
	};
	let mut cursor: Option<(DateTime<Utc>, Uuid)> = match input.before {
		Some(id) => {
			let message = lease.message(workspace, id).await?;
			let reply = threads::reply_thread(lease, id).await?;
			let valid = match &thread {
				Some(thread) => id == thread.root_message_id || reply == Some(thread.id),
				None => reply.is_none(),
			};
			if !valid {
				return Err(Error::NotFound("message unavailable".into()));
			}
			Some((message.created_at, message.id))
		}
		None => None,
	};
	let mut result = Vec::new();
	let mut scanned = 0;
	let more = loop {
		let mut query = Query::select();
		query
			.column((Alias::new("m"), Asterisk))
			.expr_as(
				Expr::col((Alias::new("c"), Alias::new("thread_id"))),
				Alias::new("reply_thread_id"),
			)
			.expr_as(
				Expr::col((Alias::new("t"), Alias::new("id"))),
				Alias::new("root_thread_id"),
			)
			.from_as(Alias::new("messages"), Alias::new("m"))
			.and_where(
				Expr::exists(
					Query::select()
						.expr(Expr::val(1))
						.from_as(Alias::new("core_records"), Alias::new("d"))
						.and_where(
							Expr::col((Alias::new("d"), Alias::new("kind"))).eq("thread_tombstone"),
						)
						.and_where(Expr::cust("d.data->>'root_message_id' = m.id::text"))
						.to_owned(),
				)
				.not(),
			)
			.join_as(
				JoinType::LeftJoin,
				Alias::new("channel_message_context"),
				Alias::new("c"),
				Expr::col((Alias::new("c"), Alias::new("message_id")))
					.equals((Alias::new("m"), Alias::new("id"))),
			)
			.join_as(
				JoinType::LeftJoin,
				Alias::new("channel_threads"),
				Alias::new("t"),
				Expr::col((Alias::new("t"), Alias::new("root_message_id")))
					.equals((Alias::new("m"), Alias::new("id"))),
			)
			.and_where(
				Expr::col((Alias::new("m"), Alias::new("workspace_id"))).eq(Expr::cust("$1")),
			)
			.cond_where(
				Condition::any()
					.add(Expr::cust("$2::timestamptz IS NULL"))
					.add(
						Expr::col((Alias::new("m"), Alias::new("created_at"))).lt(Expr::cust("$2")),
					)
					.add(
						Condition::all()
							.add(
								Expr::col((Alias::new("m"), Alias::new("created_at")))
									.eq(Expr::cust("$2")),
							)
							.add(
								Expr::col((Alias::new("m"), Alias::new("id"))).lt(Expr::cust("$3")),
							),
					),
			)
			.order_by((Alias::new("m"), Alias::new("created_at")), Order::Desc)
			.order_by((Alias::new("m"), Alias::new("id")), Order::Desc)
			.limit(100);
		if thread.is_some() {
			query.cond_where(
				Condition::any()
					.add(Expr::col((Alias::new("c"), Alias::new("thread_id"))).eq(Expr::cust("$4")))
					.add(Expr::col((Alias::new("m"), Alias::new("id"))).eq(Expr::cust("$5"))),
			);
		} else {
			query.and_where(Expr::col((Alias::new("c"), Alias::new("thread_id"))).is_null());
		}
		let sql = query.to_string(PostgresQueryBuilder);
		let mut query = sqlx::query_as::<_, Row>(&sql)
			.bind(workspace)
			.bind(cursor.map(|value| value.0))
			.bind(cursor.map(|value| value.1));
		if let Some(thread) = &thread {
			query = query.bind(thread.id).bind(thread.root_message_id);
		}
		let rows = query.fetch_all(&mut **lease.tx()).await?;
		let exhausted = rows.len() < 100;
		for row in rows {
			scanned += 1;
			cursor = Some((row.message.created_at, row.message.id));
			if lease.visible(&row.message).await? {
				result.push(ChannelMessage {
					message: row.message,
					thread_id: row.reply_thread_id.or(row.root_thread_id),
					is_thread_root: row.root_thread_id.is_some(),
					attachments: Vec::new(),
				});
				if result.len() > usize::from(input.limit) {
					break;
				}
			}
		}
		if result.len() > usize::from(input.limit) {
			break true;
		}
		if exhausted {
			break false;
		}
		// Hidden identifiers never become pagination cursors. Fail explicitly
		// rather than claim completeness after a bounded authorization scan.
		if scanned >= 5000 {
			return Err(Error::Invalid(
				"history scan limit reached; select a narrower thread".into(),
			));
		}
	};
	if more {
		result.pop();
	}
	let next_before = if more {
		result.last().map(|item| item.message.id)
	} else {
		None
	};
	result.reverse();
	let message_ids: Vec<_> = result.iter().map(|item| item.message.id).collect();
	let mut message_attachments = attachments::for_messages(lease, workspace, &message_ids).await?;
	for item in &mut result {
		item.attachments = message_attachments
			.remove(&item.message.id)
			.unwrap_or_default();
	}
	Ok(ChannelMessagePage {
		messages: result,
		next_before,
	})
}
