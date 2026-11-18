WITH config AS (SELECT *, (transaction_timestamp() AT TIME ZONE timezone)::date today FROM labor_calendar_settings WHERE singleton),
windows AS (
 SELECT a.id,s.timezone,s.today,
 CASE WHEN c.actor_id IS NULL OR dirty.actor_id IS NOT NULL OR c.calendar_revision<>s.revision THEN s.today ELSE s.today-1 END last_day,
 GREATEST(s.starts_on,(r.rating_started_at AT TIME ZONE s.timezone)::date,(a.created_at AT TIME ZONE s.timezone)::date,
 CASE WHEN c.actor_id IS NULL OR dirty.actor_id IS NOT NULL OR c.calendar_revision<>s.revision THEN s.starts_on ELSE c.last_closed+1 END) first_day
 FROM actors a JOIN actor_roles r ON r.actor_id=a.id AND r.role IN ('developer','technical_lead')
 CROSS JOIN config s LEFT JOIN developer_rating_closures c ON c.actor_id=a.id
 LEFT JOIN developer_rating_dirty dirty ON dirty.actor_id=a.id WHERE a.active AND a.kind='human'
), pending_rows AS MATERIALIZED (
 SELECT p.* FROM developer_push_activity p WHERE NOT p.processed FOR UPDATE
), pending AS (
 SELECT DISTINCT p.actor_id,(p.occurred_at AT TIME ZONE s.timezone)::date AS activity_date
 FROM pending_rows p CROSS JOIN config s
), candidates AS (
 SELECT w.id actor_id,d::date AS activity_date FROM windows w,
 LATERAL generate_series(w.first_day::timestamp,w.last_day::timestamp,interval '1 day') d
 UNION SELECT p.actor_id,p.activity_date FROM pending p JOIN windows w ON w.id=p.actor_id
 WHERE p.activity_date>=GREATEST((SELECT starts_on FROM config),
 (SELECT (r.rating_started_at AT TIME ZONE w.timezone)::date FROM actor_roles r WHERE r.actor_id=w.id))
), push_days AS (
 SELECT c.actor_id,c.activity_date FROM candidates c CROSS JOIN config s
 JOIN LATERAL (SELECT 1 FROM developer_push_activity p WHERE p.actor_id=c.actor_id
 AND p.occurred_at>=(c.activity_date::timestamp AT TIME ZONE s.timezone)
 AND p.occurred_at<((c.activity_date+1)::timestamp AT TIME ZONE s.timezone) LIMIT 1) found ON true
), author_days AS (
 SELECT c.actor_id,c.activity_date FROM candidates c CROSS JOIN config s
 WHERE EXISTS(SELECT 1 FROM git_commit_authorship g WHERE g.actor_id=c.actor_id AND g.occurred_at>=(c.activity_date::timestamp AT TIME ZONE s.timezone) AND g.occurred_at<((c.activity_date+1)::timestamp AT TIME ZONE s.timezone))
), written AS (
INSERT INTO developer_rating_entries(id,actor_id,date,source,score,reason)
SELECT 'activity:'||c.actor_id||':'||c.activity_date,c.actor_id,c.activity_date,'activity',
 CASE WHEN p.actor_id IS NOT NULL OR authored.actor_id IS NOT NULL THEN 5.0 ELSE 0.0 END,
 CASE WHEN p.actor_id IS NOT NULL THEN 'push_recorded' WHEN authored.actor_id IS NOT NULL THEN 'author_commit_recorded' ELSE 'no_push' END
FROM candidates c LEFT JOIN push_days p ON p.actor_id=c.actor_id AND p.activity_date=c.activity_date LEFT JOIN author_days authored ON authored.actor_id=c.actor_id AND authored.activity_date=c.activity_date
WHERE NOT developer_rating_excluded(c.actor_id,c.activity_date)
AND (c.activity_date<(SELECT today FROM config) OR p.actor_id IS NOT NULL OR authored.actor_id IS NOT NULL)
ON CONFLICT(actor_id,date) WHERE source='activity' DO UPDATE SET score=EXCLUDED.score,reason=EXCLUDED.reason
WHERE developer_rating_entries.score<>EXCLUDED.score OR developer_rating_entries.reason<>EXCLUDED.reason
RETURNING 1
), consumed AS (
 UPDATE developer_push_activity p SET processed=true FROM pending_rows claimed
 WHERE p.actor_id=claimed.actor_id AND p.provider=claimed.provider AND p.scope_id=claimed.scope_id
 AND p.delivery_id=claimed.delivery_id AND p.project_id=claimed.project_id RETURNING 1
)
 , pruned AS (
 DELETE FROM developer_rating_entries e USING config s WHERE e.source='activity' AND e.date=s.today
 AND NOT EXISTS(SELECT 1 FROM git_commit_authorship g WHERE g.actor_id=e.actor_id AND g.occurred_at>=(e.date::timestamp AT TIME ZONE s.timezone) AND g.occurred_at<((e.date+1)::timestamp AT TIME ZONE s.timezone))
 AND NOT EXISTS(SELECT 1 FROM developer_push_activity p WHERE p.actor_id=e.actor_id
 AND p.occurred_at>=(e.date::timestamp AT TIME ZONE s.timezone)
 AND p.occurred_at<((e.date+1)::timestamp AT TIME ZONE s.timezone)) RETURNING 1
)
SELECT (SELECT count(*) FROM written)+(SELECT count(*) FROM pruned)
