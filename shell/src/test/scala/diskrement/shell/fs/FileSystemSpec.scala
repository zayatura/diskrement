package diskrement.shell.fs

import zio._
import zio.test._
import java.nio.file.{Files, Path}

object FileSystemSpec extends ZIOSpecDefault:

  /** root/{a.txt, .hidden, sub/b.txt, sub/c.txt} plus a symlink that must be
    * skipped. Deleted recursively on release. */
  private val makeTree: ZIO[Scope, Throwable, Path] =
    ZIO.acquireRelease(
      ZIO.attempt {
        val root = Files.createTempDirectory("fsspec")
        Files.writeString(root.resolve("a.txt"), "aaa")
        Files.writeString(root.resolve(".hidden"), "h")
        val sub = Files.createDirectory(root.resolve("sub"))
        Files.writeString(sub.resolve("b.txt"), "bbbb")
        Files.writeString(sub.resolve("c.txt"), "ccccc")
        try Files.createSymbolicLink(root.resolve("link.txt"), root.resolve("a.txt"))
        catch case _: Throwable => ()   // symlink creation may be unsupported; fine
        root
      })(root =>
        ZIO.attempt(
          Files.walk(root).sorted(java.util.Comparator.reverseOrder())
            .forEach(p => Files.deleteIfExists(p))).orDie)

  def spec = suite("FileSystemLive")(
    test("walk returns regular files (incl dotfiles); skips symlinks and dirs"):
      ZIO.scoped {
        for
          root  <- makeTree
          paths <- ZIO.serviceWithZIO[FileSystem](_.walk(root))
          rels   = paths.map(p => root.relativize(p).toString).sorted
        yield assertTrue(rels == List(".hidden", "a.txt", "sub/b.txt", "sub/c.txt"))
      },
    test("stat returns size and an mtime truncated to millis"):
      ZIO.scoped {
        for
          root <- makeTree
          st   <- ZIO.serviceWithZIO[FileSystem](_.stat(root.resolve("sub/c.txt")))
        yield assertTrue(
          st.sizeBytes == 5L,
          st.mtime == st.mtime.truncatedTo(java.time.temporal.ChronoUnit.MILLIS))
      }
  ).provideLayer(FileSystem.live)
