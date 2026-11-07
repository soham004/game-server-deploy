# Feature: Java Runtime Management

## Why
Wrong Java is the #1 "server won't start" cause. MC 26.x needs Java **25**; 1.20.5–1.21.11 needs 21; 1.18–1.20.4
needs 17; ≤1.16 needs 8. The required major comes from the Mojang per-version JSON (`javaVersion.majorVersion`).

## Detection (gsd-core/java/detect.rs)
Scan: `JAVA_HOME`, `PATH`, the Windows registry (JavaSoft, Eclipse Adoptium, Microsoft, Azul, BellSoft keys), common dirs
(`C:\Program Files\{Java,Eclipse Adoptium,Microsoft,Zulu}\*`, `/usr/lib/jvm/*`, `/opt/*`,
`/Library/Java/JavaVirtualMachines/*/Contents/Home`, `~/.sdkman/candidates/java/*`), plus managed `<data>/runtimes/java/*`.
For each: read `<home>/release` (`JAVA_VERSION`, `IMPLEMENTOR`, `OS_ARCH`). If it's missing, run
`java -XshowSettings:properties -version` (stderr) with a timeout. Dedupe by canonical path.
Result: `JavaRuntime { id, path, major, full_version, vendor, arch, managed: bool }`.

## Download (job)
Adoptium `assets/latest/{major}/hotspot?os&architecture&image_type=jre&vendor=eclipse` → download the archive → verify the
sha256 checksum → extract into `runtimes/java/temurin-{major}-{arch}/` (strip the top folder) → verify by running `java -version`.
If no JRE image exists for a platform or major, fall back to `image_type=jdk`.

## Selection
`java.runtime = "auto"` (default) picks a **managed runtime of the exact required major**, else any detected one of the
exact major, else offers to download. (We prefer the exact major because newer Javas can break older modded servers.)
The user can pin a specific runtime or path.

## JVM presets
- `aikar`: G1 tuning flags (well-known set, used by Paper docs) for heaps ≤ 12 GB.
- `zgc`: `-XX:+UseZGC -XX:+ZGenerational` (ZGenerational is the default on recent JDKs; flag harmless or dropped for 25 (verify)) for large heaps.
- `none`: just memory flags. `custom`: free text.
Memory slider: default `min(4 GB, 50% host RAM)`, warns if it exceeds 80% of host RAM.
