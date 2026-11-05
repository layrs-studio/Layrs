-- Stable insertion order; timestamps and verdicts do not control pagination.
CREATE SEQUENCE layrs_collaboration.run_list_sequence;
ALTER TABLE layrs_collaboration.runs ADD COLUMN list_sequence BIGINT;
WITH ordered AS (
    SELECT run_id,row_number() OVER (ORDER BY created_at,run_id) AS ordinal
    FROM layrs_collaboration.runs
)
UPDATE layrs_collaboration.runs r SET list_sequence=o.ordinal FROM ordered o WHERE r.run_id=o.run_id;
SELECT setval('layrs_collaboration.run_list_sequence',greatest(coalesce(max(list_sequence),0),1),count(*)>0)
FROM layrs_collaboration.runs;
ALTER TABLE layrs_collaboration.runs ALTER COLUMN list_sequence SET NOT NULL;
ALTER TABLE layrs_collaboration.runs ALTER COLUMN list_sequence SET DEFAULT nextval('layrs_collaboration.run_list_sequence');
ALTER SEQUENCE layrs_collaboration.run_list_sequence OWNED BY layrs_collaboration.runs.list_sequence;
CREATE UNIQUE INDEX runs_list_sequence_unique ON layrs_collaboration.runs(list_sequence);
CREATE INDEX runs_space_list_order ON layrs_collaboration.runs(space_id,list_sequence DESC);
DO $run_list_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT USAGE,SELECT ON SEQUENCE layrs_collaboration.run_list_sequence TO %I',writer);
END
$run_list_grants$;
