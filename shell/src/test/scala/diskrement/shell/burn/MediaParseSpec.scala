package diskrement.shell.burn

import zio.test._

object MediaParseSpec extends ZIOSpecDefault:

  private val blankFixture = List(
    "xorriso 1.5.8.pl02 : RockRidge filesystem manipulator, libburnia project.",
    "Drive current: -outdev 'stdio:empty.iso'",
    "Media current: stdio file, overwriteable",
    "Media status : is blank",
    "Media summary: 0 sessions, 0 data blocks, 0 data,  660g free",
    "Media space  : 346016270s",
    "After commit : 346016270s")

  private val appendableFixture = List(
    "Media current: stdio file, overwriteable",
    "Media status : is written , is appendable",
    "ISO session  :   1 ,        32 ,        29s , ISOIMAGE",
    "ISO session  :   2 ,        64 ,        28s , ISOIMAGE",
    "Media summary: 2 sessions, 57 data blocks,  114k data,  660g free",
    "Media space  : 346016430s")

  private val closedFixture = List(
    "Media current: BD-R sequential recording",
    "Media status : is written , is closed",
    "Media summary: 3 sessions, 57 data blocks,  114k data, 0 free",
    "Media space  : 0s")

  // Real captured output (2026-08-30, Pioneer BDR-UD04, a POW-formatted BD-R):
  // such media cannot take sequential multisession burns and must be refused clearly.
  private val unsuitableFixture = List(
    "Media current: BD-R sequential recording, Pseudo Overwrite formatted",
    "Media status : is unsuitable , is POW formatted",
    "Media summary: unsuitable Pseudo Overwrite formatted BD-R",
    "Media space  : 248768s")

  def spec = suite("MediaParseSpec")(
    test("blank media") {
      assertTrue(MediaParse.parse(blankFixture) ==
        Right(RawMediaInfo(RawStatus.Blank, 0, 346016270L * 2048)))
    },
    test("appendable media with session count") {
      assertTrue(MediaParse.parse(appendableFixture) ==
        Right(RawMediaInfo(RawStatus.Appendable, 2, 346016430L * 2048)))
    },
    test("written but not appendable parses as Closed") {
      assertTrue(MediaParse.parse(closedFixture) == Right(RawMediaInfo(RawStatus.Closed, 3, 0L)))
    },
    test("POW-formatted BD-R parses as Unsuitable") {
      assertTrue(MediaParse.parse(unsuitableFixture) ==
        Right(RawMediaInfo(RawStatus.Unsuitable, 0, 248768L * 2048)))
    },
    test("no recognizable status is an error") {
      assertTrue(MediaParse.parse(List("Drive current: ...")).isLeft)
    },
    test("failure classification recognizes no-media, permission, missing device") {
      assertTrue(
        MediaParse.classifyFailure(List("libburn : SORRY : No disc loaded, not ready")) == FailureKind.NoMediaKind,
        MediaParse.classifyFailure(List("libburn : NOTE : Failed to open device '/dev/sr0' : Permission denied")) == FailureKind.PermissionKind,
        MediaParse.classifyFailure(List("stdio: could not open, No such file or directory")) == FailureKind.NotFoundKind,
        MediaParse.classifyFailure(List("xorriso : FAILURE : something else")) == FailureKind.OtherKind)
    }
  )
