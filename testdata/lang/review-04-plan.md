# Plan review: queue redesign

Summary: The plan is generally solid and well-structured, but it seamlessly glosses over the migration of existing tickets, which could be a showstopper for users who are mid-run during the upgrade.

The plan proposes to replace the file-per-ticket layout with a single SQLite database. However, it does not specify how running tickets written by the old version are handled, nor what happens if two versions run side by side.

Recommendation: Add a migration section that defines the order of operations, i.e. stop new enqueues, drain the running tickets, then switch. Make sure that a downgrade path exists.
