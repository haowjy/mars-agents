# Policy resolution

Harness precedence is CLI, overlay, overlay model policy, profile, profile model policy, settings model policy, then alias. Resolve it independently of model precedence: a CLI model does not erase a profile harness preference. A blocked CLI harness still permits implicit model backups on that harness; a CLI model pin prevents backups.

Keep each model attempt's route, settings and provenance together. If no eligible or supported preferred route wins, select the first whole unverified attempt, not a reconstruction from the last attempt. Append every trace to the aggregate report before moving on; a configuration rejection before assessment is an unassessed attempt, not a fabricated verdict.
