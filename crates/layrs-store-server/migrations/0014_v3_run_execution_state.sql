-- Legacy /runs registrations already declare execution in progress. Preserve
-- that contract; /runs/pending explicitly waits for the producer's begin signal.
ALTER TABLE layrs_collaboration.runs
    ADD COLUMN execution_requested BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN execution_started_at TIMESTAMPTZ,
    ADD CONSTRAINT run_execution_signal_shape CHECK (
        execution_requested OR execution_started_at IS NULL
    ),
    ADD CONSTRAINT run_pending_has_no_verdict CHECK (
        NOT execution_requested OR execution_started_at IS NOT NULL OR proof_object_id IS NULL
    );
