package diskrement.shell.fs

import zio._
import zio.test._
import java.nio.file.{Files, Path, Paths}

object HasherSpec extends ZIOSpecDefault:
  // Known SHA-256 vectors:
  //   sha256("")    = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
  //   sha256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad

  private def withFile[A](content: Array[Byte])(
      use: Path => ZIO[Hasher, Throwable, A]): ZIO[Hasher, Throwable, A] =
    ZIO.acquireReleaseWith(
      ZIO.attempt { val p = Files.createTempFile("hashspec", ".bin"); Files.write(p, content); p })(
      p => ZIO.attempt(Files.deleteIfExists(p)).orDie)(use)

  def spec = suite("Hasher")(
    test("hashes an empty file to the SHA-256 of empty"):
      withFile(Array.emptyByteArray) { p =>
        for h <- ZIO.serviceWithZIO[Hasher](_.hash(p))
        yield assertTrue(
          h.value == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
      },
    test("hashes \"abc\""):
      withFile("abc".getBytes("UTF-8")) { p =>
        for h <- ZIO.serviceWithZIO[Hasher](_.hash(p))
        yield assertTrue(
          h.value == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
      },
    test("chunked digest matches a one-shot digest for a large input"):
      val big = Array.tabulate(300_000)(i => (i % 251).toByte)
      val expected =
        java.security.MessageDigest.getInstance("SHA-256").digest(big)
          .map(b => f"${b & 0xff}%02x").mkString
      withFile(big) { p =>
        for h <- ZIO.serviceWithZIO[Hasher](_.hash(p))
        yield assertTrue(h.value == expected)
      },
    test("fails with IOException on a non-existent file"):
      ZIO.serviceWithZIO[Hasher](_.hash(Paths.get("/tmp/diskrement-hasher-does-not-exist-xyz.bin")))
        .flip.map(e => assertTrue(e.isInstanceOf[java.io.IOException]))
  ).provideLayer(Hasher.live)
