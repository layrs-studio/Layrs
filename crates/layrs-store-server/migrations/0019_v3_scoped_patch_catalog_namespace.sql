-- Keep extension tables outside the immutable baseline catalog fingerprint.
-- PostgreSQL preserves their foreign keys, indexes and grants during the move.
ALTER TABLE scoped_patch_sessions SET SCHEMA layrs_collaboration;
ALTER TABLE scoped_patch_chunks SET SCHEMA layrs_collaboration;
ALTER TABLE scoped_patch_reserved_objects SET SCHEMA layrs_collaboration;
ALTER TABLE scoped_patch_candidates SET SCHEMA layrs_collaboration;
