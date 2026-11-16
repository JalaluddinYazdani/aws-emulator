// SPDX-License-Identifier: CC0-1.0

//! HOW LONG a key of a substitution table may be, driven end to end.
//!
//! RULE under test: "The maximum length of any single expression attribute name or
//! expression attribute value is 255 bytes" (Service, account, and table quotas) — the bound
//! is on the `#name`/`:value` TOKEN itself, prefix included. A key over it is refused as the
//! invalid key of the member that holds it, named by its own byte length rather than by the
//! key (which no caller could read back at that size):
//! `<member> contains invalid key: The expression attribute map contains a key that is too
//! long; size of key: <n>`. The sentence carries the `<N> validation error(s) detected: `
//! envelope exactly where every other refusal about a request's expressions does
//! (`unused_substitution_key_envelopes` spans that rule): behind `UpdateExpression` and
//! `ConditionExpression`, and on its own behind a member that only narrows a read.
//!
//! The rows span what the rule generalises over: the table the key sits in
//! (`ExpressionAttributeNames`, `ExpressionAttributeValues`), the expression member the key
//! was offered to (an update, a condition, a key condition, a projection), the operation
//! (an item update, an item write, a query, a scan) and how far over the bound the key is
//! (one byte, many). The boundary itself is driven as the control: a key of exactly 255
//! bytes is served, so the refusals are about the SIZE and not about carrying a long key
//! at all. The last assertion pins the precedence between this rule and the spelling rule
//! beside it: a key that is both misspelled and over-long earns the spelling sentence.
//!
//! Every call is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;
// WHICH expression members carry the envelope is stated ONCE for this crate.
use crate::undefined_expression_tokens::envelopes;

/// The table every row below drives: one string partition key.
const TABLE: &str = "SubstitutionKeySizeBound";

/// The bound under test, in bytes, as the quota states it.
const MAX_KEY_BYTES: usize = 255;

/// The key of the seeded item, so no row is refused for its key instead of for its
/// substitution tables.
fn key() -> Value {
    json!({"pk": {"S": "item-1"}})
}

/// A `prefix`-spelled substitution token of exactly `bytes` bytes: the prefix and then
/// as many `v`s as fill the rest, each one byte, so the length a row names is the length
/// the service measures.
fn token_of(prefix: char, bytes: usize) -> String {
    format!("{prefix}{}", "v".repeat(bytes - 1))
}

/// The sentence a key of `size` bytes in `table` earns, as the request that offered it to
/// `expression_member` states it: inside the envelope where that member's refusals carry
/// one, and on its own otherwise.
fn too_long(expression_member: &str, table: &str, size: usize) -> String {
    let stated = format!(
        "{table} contains invalid key: The expression attribute map contains a key that is \
         too long; size of key: {size}"
    );
    if envelopes(expression_member) {
        return format!("1 validation error detected: {stated}");
    }
    stated
}

/// The table and the one item every row is driven against.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE,
               "Item": {"pk": {"S": "item-1"}, "status": {"S": "active"}, "note": {"S": "kept"}}}),
    )
    .await;
    router
}

/// One row: the case, the operation, the request, the expression member the over-long key
/// was offered to, the substitution table it sits in, and its size in bytes.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    expression_member: &'static str,
    table: &'static str,
    size: usize,
}

fn rows() -> Vec<Row> {
    let value_256 = token_of(':', 256);
    let name_256 = token_of('#', 256);
    let name_300 = token_of('#', 300);
    let value_1024 = token_of(':', 1024);
    vec![
        Row {
            // The report this suite answers: `SET field = :vvvv…vvvv`, one byte over.
            case: "a value key one byte over the bound, in an update",
            operation: "UpdateItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "UpdateExpression": format!("SET note = {value_256}"),
                            "ExpressionAttributeValues": {&value_256: {"S": "changed"}}}),
            expression_member: "UpdateExpression",
            table: "ExpressionAttributeValues",
            size: 256,
        },
        Row {
            case: "a name key one byte over the bound, in an update",
            operation: "UpdateItem",
            request: json!({"TableName": TABLE, "Key": key(),
                            "UpdateExpression": format!("SET {name_256} = :v"),
                            "ExpressionAttributeNames": {&name_256: "note"},
                            "ExpressionAttributeValues": {":v": {"S": "changed"}}}),
            expression_member: "UpdateExpression",
            table: "ExpressionAttributeNames",
            size: 256,
        },
        Row {
            case: "a name key far over the bound, in a write's condition",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": {"pk": {"S": "item-2"}, "status": {"S": "paused"}},
                            "ConditionExpression": format!("attribute_not_exists({name_300})"),
                            "ExpressionAttributeNames": {&name_300: "status"}}),
            expression_member: "ConditionExpression",
            table: "ExpressionAttributeNames",
            size: 300,
        },
        Row {
            case: "a value key far over the bound, in a query's key condition",
            operation: "Query",
            request: json!({"TableName": TABLE,
                            "KeyConditionExpression": format!("pk = {value_1024}"),
                            "ExpressionAttributeValues": {&value_1024: {"S": "item-1"}}}),
            expression_member: "KeyConditionExpression",
            table: "ExpressionAttributeValues",
            size: 1024,
        },
        Row {
            case: "a name key one byte over the bound, in a scan's projection",
            operation: "Scan",
            request: json!({"TableName": TABLE, "ConsistentRead": true,
                            "ProjectionExpression": name_256.clone(),
                            "ExpressionAttributeNames": {&name_256: "status"}}),
            expression_member: "ProjectionExpression",
            table: "ExpressionAttributeNames",
            size: 256,
        },
    ]
}

#[tokio::test]
async fn a_substitution_key_over_the_bound_is_refused_by_its_size() {
    let router = seeded().await;

    for row in rows() {
        let expected = too_long(row.expression_member, row.table, row.size);
        let (status, body) = call(&router, row.operation, row.request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {:?} ({}): expected 400 ValidationException, got {status} {body}",
            row.case,
            row.operation
        );
        let said = message(&body);
        assert_eq!(
            said, expected,
            "case {:?} ({}): expected {expected:?}, got {said:?}",
            row.case, row.operation
        );
        // Only the members a WRITE states its intent through are stated inside the
        // envelope, so a read's refusal must not carry it at all.
        assert_eq!(
            said.starts_with("1 validation error detected: "),
            envelopes(row.expression_member),
            "case {:?} ({}): the envelope follows the expression member, got {said:?}",
            row.case,
            row.operation
        );
    }

    // Every refusal above left the store where it was: the seeded item still carries the
    // values it was written with, and the item a refused write would have added is absent.
    let read = ok(&router, "GetItem", json!({"TableName": TABLE, "Key": key()})).await;
    assert_eq!(
        read["Item"]["note"],
        json!({"S": "kept"}),
        "the refused requests must leave the item alone, got {read}"
    );
    let absent = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "item-2"}}}),
    )
    .await;
    assert_eq!(
        absent.get("Item"),
        None,
        "a refused write must store nothing, got {absent}"
    );
}

#[tokio::test]
async fn a_substitution_key_at_the_bound_is_served() {
    let router = seeded().await;

    // The control for every row above: the same requests with each key at EXACTLY the
    // bound are served, so the refusals are about the size and not about a long key.
    let name_255 = token_of('#', MAX_KEY_BYTES);
    let value_255 = token_of(':', MAX_KEY_BYTES);
    let updated = ok(
        &router,
        "UpdateItem",
        json!({"TableName": TABLE, "Key": key(),
               "UpdateExpression": format!("SET {name_255} = {value_255}"),
               "ExpressionAttributeNames": {&name_255: "note"},
               "ExpressionAttributeValues": {&value_255: {"S": "changed"}},
               "ReturnValues": "ALL_NEW"}),
    )
    .await;
    assert_eq!(
        updated["Attributes"]["note"],
        json!({"S": "changed"}),
        "an update whose keys sit at the bound must apply, got {updated}"
    );
    let queried = ok(
        &router,
        "Query",
        json!({"TableName": TABLE,
               "KeyConditionExpression": format!("{name_255} = {value_255}"),
               "ExpressionAttributeNames": {&name_255: "pk"},
               "ExpressionAttributeValues": {&value_255: {"S": "item-1"}}}),
    )
    .await;
    assert_eq!(
        queried["Count"],
        json!(1),
        "a query whose keys sit at the bound must report the seeded item, got {queried}"
    );
}

#[tokio::test]
async fn a_misspelled_key_over_the_bound_earns_the_spelling_sentence() {
    let router = seeded().await;

    // Spelling is asked of every key before size is: a key that is no token at all defines
    // nothing whatever its length, so it is that defect the caller is told about.
    let unprefixed = "v".repeat(256);
    let (status, body) = call(
        &router,
        "UpdateItem",
        json!({"TableName": TABLE, "Key": key(),
               "UpdateExpression": "SET note = :v",
               "ExpressionAttributeValues": {&unprefixed: {"S": "changed"}}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a misspelled over-long key is a 400 ValidationException, got {status} {body}"
    );
    assert_eq!(
        message(&body),
        format!(
            "1 validation error detected: ExpressionAttributeValues contains invalid key: \
             Syntax error; key: \"{unprefixed}\""
        ),
        "the spelling sentence takes precedence over the size one, got {body}"
    );
}
