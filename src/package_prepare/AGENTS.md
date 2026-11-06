# Package preparation

`DiskTreeSource` latches IO and containment failures because `TreeSource` has no
error channel. Check the latched error immediately after preparation.
