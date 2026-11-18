use super::TaskContributorRating;
use serde_json::{Value, json};
#[allow(clippy::duplicate_mod)]
#[path = "../tests/support/work_item_fixture.rs"]
mod work_item_fixture;
#[test]
#[ignore = "requires PostgreSQL; durable private rating claims use the integration fixture"]
fn durable_claims_never_repeat_tokens_and_only_current_developers_receive_private_points() {
    let mut f = work_item_fixture::WorkItemFixture::new();
    let task = f.task("Private rating fixture");
    let actor = f.ctx.actor_id().to_owned();
    let project = f.project.id.clone();
    f.store.client.execute("INSERT INTO git_connections(id,provider,host,owner_actor_id,remote_account_id,remote_login,status,config_revision,tokens,expires_at) VALUES('rating-connection','gitlab','fixture.local',$1,'1','fixture','active',1,'{}',now()+interval '1 hour')",&[&actor]).unwrap();
    f.store.client.execute("INSERT INTO project_repositories(id,project_id,provider,host,requested_full_name,remote_id,remote_full_name,connection_id,authorization_status) VALUES('repo_rating',$1,'gitlab','fixture.local','o/r','123','o/r','rating-connection','authorized')",&[&project]).unwrap();
    f.store.client.execute("INSERT INTO task_transition_runs(id,task_id,project_id,actor_id,credential_id,auth_method,task_revision,transition_id,fingerprint,parameters,record) VALUES('rating-run',$1,$2,$3,'fixture','session',$4,'finish','fixture','{}','{}')",&[&task.id,&project,&actor,&task.revision]).unwrap();
    let authority = crate::task_transition_provider::TaskTransitionProvider::authority(
        &mut f.store,
        &f.ctx,
        Some(&project),
        "repo_rating",
    )
    .unwrap();
    for (index, review) in [
        "review-interrupted",
        "review-completed",
        "review-manager",
        "review-no-score",
        "review-revoked",
    ]
    .into_iter()
    .enumerate()
    {
        f.store.client.execute("INSERT INTO task_pre_pr_reviews(id,run_id,task_id,task_revision,project_id,repository_id,head_sha,base_sha,requested_by,authority,input,status,report) VALUES($1,'rating-run',$2,$3,$4,'repo_rating',$5,$6,$7,'{}','{}','completed','{}')",&[&review,&task.id,&task.revision,&project,&"a".repeat(40),&format!("{:040x}",index+1),&actor]).unwrap();
    }
    f.store
        .client
        .execute("UPDATE task_pre_pr_reviews SET authority=$1", &[&authority])
        .unwrap();
    let (interrupted, _) = TaskContributorRating::claim(
        &mut f.store,
        "review-interrupted",
        &actor,
        "fingerprint",
        &json!({}),
        "fixture-model",
    )
    .unwrap();
    assert!(
        TaskContributorRating::claim(
            &mut f.store,
            "review-interrupted",
            &actor,
            "fingerprint",
            &json!({}),
            "fixture-model"
        )
        .is_err()
    );
    let status: String = f
        .store
        .client
        .query_one(
            "SELECT status FROM task_contributor_ratings WHERE id=$1",
            &[&interrupted],
        )
        .unwrap()
        .get(0);
    assert_eq!(status, "reconcile_required");
    f.store
        .client
        .execute(
            "UPDATE actor_roles SET role='developer' WHERE actor_id=$1",
            &[&actor],
        )
        .unwrap();
    f.store.client.execute("INSERT INTO git_author_aliases(id,actor_id,provider,host,email,created_by) VALUES('rating-alias',$1,'gitlab','fixture.local','dev@example.com',$1)",&[&actor]).unwrap();
    let rr: i64 = f
        .store
        .client
        .query_one(
            "SELECT revision FROM actor_roles WHERE actor_id=$1",
            &[&actor],
        )
        .unwrap()
        .get(0);
    let input = json!({"organization_knowledge":[],"primary_knowledge":null,"criteria":[],"commits":[{"sha":"a".repeat(40),"files":[{"path":"a.rs","patch":"@@ -1 +1 @@\n-old();\n+new();"}]}],"attribution":[{"active_alias_id":"rating-alias","alias_revision":1,"role_revision":rr}]});
    f.store.client.execute("INSERT INTO git_commit_authorship(id,provider,host,remote_repository_id,repository_id,project_id,commit_sha,author_name,author_email,actor_id,alias_id,scope_id,delivery_id) VALUES('rating-commit','gitlab','fixture.local','123','repo_rating',$1,$2,'Author','dev@example.com',$3,'rating-alias','fixture','fixture')",&[&project,&"a".repeat(40),&actor]).unwrap();
    let mut commits = input["commits"].clone();
    commits[0]["author_email"] = json!("dev@example.com");
    let credited = crate::task_contributor_code::TaskContributorCode::contributors(
        &mut f.store,
        &project,
        "repo_rating",
        "gitlab",
        "fixture.local",
        "123",
        &commits,
    )
    .unwrap();
    assert_eq!(credited.len(), 1);
    assert_eq!(credited[0]["actor_id"], actor);
    let mut unknown = commits.clone();
    unknown[0]["author_email"] = json!("unknown@example.com");
    assert!(
        crate::task_contributor_code::TaskContributorCode::contributors(
            &mut f.store,
            &project,
            "repo_rating",
            "gitlab",
            "fixture.local",
            "123",
            &unknown
        )
        .unwrap()
        .is_empty()
    );
    let dimensions:Vec<Value>=["correctness","security","maintainability"].into_iter().map(|dimension|json!({"dimension":dimension,"score":4.0,"reason":"Static changed-line evidence.","evidence":[{"commit_sha":"a".repeat(40),"path":"a.rs","side":"new","line":1,"quote":"new();"}]})).collect();
    let report = json!({"status":"rated","reason":"Private contributor assessment.","limitations":["Tests not executed."],"rubric":dimensions});
    let (id, _) = TaskContributorRating::claim(
        &mut f.store,
        "review-completed",
        &actor,
        "fp2",
        &input,
        "fixture-model",
    )
    .unwrap();
    assert!(
        TaskContributorRating::finish(
            &mut f.store,
            &f.ctx,
            &id,
            "review-completed",
            &actor,
            &project,
            &input,
            &report
        )
        .unwrap()
    );
    let row = f
        .store
        .client
        .query_one(
            "SELECT source,score,created_by FROM developer_rating_entries WHERE id=$1",
            &[&format!("contributor-ai:{id}")],
        )
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "ai");
    assert_eq!(row.get::<_, f64>(1), 4.0);
    assert!(row.get::<_, Option<String>>(2).is_none());
    let (_, previous) = TaskContributorRating::claim(
        &mut f.store,
        "review-completed",
        &actor,
        "fp2",
        &input,
        "fixture-model",
    )
    .unwrap();
    assert_eq!(previous.as_deref(), Some("completed"));
    assert!(
        TaskContributorRating::claim(
            &mut f.store,
            "review-completed",
            &actor,
            "changed",
            &input,
            "fixture-model"
        )
        .is_err()
    );
    let (no_score, _) = TaskContributorRating::claim(
        &mut f.store,
        "review-no-score",
        &actor,
        "fp-none",
        &input,
        "fixture-model",
    )
    .unwrap();
    assert!(!TaskContributorRating::finish(&mut f.store,&f.ctx,&no_score,"review-no-score",&actor,&project,&input,&json!({"status":"insufficient_evidence","reason":"Cannot justify a score from these lines.","limitations":[],"rubric":[]})).unwrap());
    let (revoked, _) = TaskContributorRating::claim(
        &mut f.store,
        "review-revoked",
        &actor,
        "fp-revoked",
        &input,
        "fixture-model",
    )
    .unwrap();
    f.store.client.execute("UPDATE git_author_aliases SET revoked_at=now(),revoked_by=$1,revocation_reason='Fixture revocation' WHERE id='rating-alias'",&[&actor]).unwrap();
    assert!(
        TaskContributorRating::finish(
            &mut f.store,
            &f.ctx,
            &revoked,
            "review-revoked",
            &actor,
            &project,
            &input,
            &report
        )
        .is_err()
    );
    f.store
        .client
        .execute(
            "UPDATE actor_roles SET role='manager' WHERE actor_id=$1",
            &[&actor],
        )
        .unwrap();
    let (id, _) = TaskContributorRating::claim(
        &mut f.store,
        "review-manager",
        &actor,
        "fp3",
        &input,
        "fixture-model",
    )
    .unwrap();
    assert!(
        TaskContributorRating::finish(
            &mut f.store,
            &f.ctx,
            &id,
            "review-manager",
            &actor,
            &project,
            &input,
            &report
        )
        .is_err()
    );
    assert_eq!(
        f.store
            .client
            .query_one(
                "SELECT count(*) FROM developer_rating_entries WHERE source='ai'",
                &[]
            )
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}

#[test]
fn assessment_reuses_latest_preceding_parent_target_review_not_stage_base() {
    let run = json!({"outcomes":[{"kind":"create_code_review","result":{"reviews":[{"id":"stage-review","repository_id":"repo_parent","head_sha":"head","base_sha":"develop-base","status":"completed"}]}},{"kind":"create_code_review","result":{"reviews":[{"id":"other-repository","repository_id":"repo_other","head_sha":"head","base_sha":"other","status":"completed"},{"id":"parent-review","repository_id":"repo_parent","head_sha":"head","base_sha":"parent-feature-base","status":"completed"}]}}]});
    let exact = TaskContributorRating::preceding_review(&run, "repo_parent", "head").unwrap();
    assert_eq!(exact["id"], "parent-review");
    assert_eq!(exact["base_sha"], "parent-feature-base");
    assert!(TaskContributorRating::preceding_review(&run, "repo_parent", "new-head").is_err());
}
