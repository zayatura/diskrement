package diskrement.shell

import zio._
import zio.test._

object ProgressSinkSpec extends ZIOSpecDefault:
  private final case class RecordingSink(ref: Ref[List[String]]) extends ProgressSink:
    def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit] =
      ref.update(_ :+ s"start:$label")
    def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit] =
      ref.update(_ :+ "advance")
    def done: UIO[Unit] = ref.update(_ :+ "done")

  private def recorded[E, A](use: ProgressSink => ZIO[Any, E, A]): UIO[List[String]] =
    for
      ref  <- Ref.make(List.empty[String])
      sink  = RecordingSink(ref)
      _    <- use(sink).exit
      evs  <- ref.get
    yield evs

  def spec = suite("ProgressSink.phase")(
    test("brackets a successful body"):
      recorded(s => s.phase("hashing", 10, 1)(s.advance(10, 1)))
        .map(evs => assertTrue(evs == List("start:hashing", "advance", "done")))
    ,
    test("runs done on typed failure and on defect"):
      for
        f <- recorded(s => s.phase("p", 0, 0)(ZIO.fail("boom")))
        d <- recorded(s => s.phase("p", 0, 0)(ZIO.die(new RuntimeException("boom"))))
      yield assertTrue(f == List("start:p", "done"), d == List("start:p", "done"))
    ,
    test("runs done when a parallel body is interrupted"):
      for
        ref    <- Ref.make(List.empty[String])
        sink    = RecordingSink(ref)
        gate   <- Promise.make[Nothing, Unit]
        fiber  <- sink.phase("p", 0, 0) {
                    ZIO.foreachPar(1 to 4)(_ => gate.succeed(()) *> ZIO.never)
                      .withParallelism(4)
                  }.fork
        _      <- gate.await
        _      <- fiber.interrupt
        evs    <- ref.get
      yield assertTrue(evs.head == "start:p", evs.last == "done")
    ,
    test("a sink runs several phases in sequence"):
      recorded(s => s.phase("a", 0, 0)(ZIO.unit) *> s.phase("b", 0, 0)(ZIO.unit))
        .map(evs => assertTrue(evs == List("start:a", "done", "start:b", "done")))
  )
