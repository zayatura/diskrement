package diskrement.core

class ChangeSetTest extends munit.FunSuite:
  test("ChangeSet.empty has no entries and reports nothing to do"):
    assert(ChangeSet.empty.isEmpty)

  test("a ChangeSet with any added/modified/deleted is non-empty"):
    val cs = ChangeSet.empty.copy(deleted = List(RelPath("gone")))
    assert(!cs.isEmpty)
