# One owned child per invocation. JSON request on stdin; EOF cancels that child.
# No shell evaluation, environment dump, service, process-name lookup, or dependency.
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
try {
    $request = [Console]::ReadLine() | ConvertFrom-Json
    if ($null -eq $request) { throw 'Missing process measurement request' }
    Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

public static class WorktreeProcessProbe {
    [StructLayout(LayoutKind.Sequential)]
    public struct MemoryCounters {
        public uint cb, faults;
        public UIntPtr peakWorkingSet, workingSet, peakPagedPool, pagedPool;
        public UIntPtr peakNonpagedPool, nonpagedPool, pagefile, peakPagefile, privateBytes;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct IoCounters {
        public ulong readOperations, writeOperations, otherOperations, readBytes, writeBytes, otherBytes;
    }
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetProcessTimes(IntPtr handle, out long created, out long exited, out long kernel, out long user);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetProcessIoCounters(IntPtr handle, out IoCounters counters);
    [DllImport("psapi.dll", SetLastError = true)]
    static extern bool GetProcessMemoryInfo(IntPtr handle, out MemoryCounters counters, uint size);

    // Microsoft CRT quoting: quote every argument, double trailing backslashes,
    // and double backslashes before an embedded quote, then escape that quote.
    static string Quote(string argument) {
        if (argument == null || argument.IndexOf('\0') >= 0) throw new ArgumentException("Invalid argv element");
        var output = new StringBuilder("\"");
        int slashes = 0;
        foreach (char c in argument) {
            if (c == '\\') { slashes++; continue; }
            output.Append('\\', c == '"' ? slashes * 2 + 1 : slashes);
            output.Append(c); slashes = 0;
        }
        return output.Append('\\', slashes * 2).Append('"').ToString();
    }
    sealed class Capture {
        public long bytes;
        public volatile bool exceeded;
        public volatile string error;
        public Task task;
    }
    static async Task Drain(Stream source, string path, int limit, Capture capture) {
        try {
            using (var destination = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.Read)) {
                var buffer = new byte[8192];
                int length;
                while ((length = await source.ReadAsync(buffer, 0, buffer.Length).ConfigureAwait(false)) != 0) {
                    long previous = Interlocked.Read(ref capture.bytes);
                    int retained = (int)Math.Max(0, Math.Min(length, limit - previous));
                    if (retained > 0) destination.Write(buffer, 0, retained);
                    if (Interlocked.Add(ref capture.bytes, length) > limit) capture.exceeded = true;
                }
            }
        } catch (Exception error) { capture.error = error.Message; }
    }
    static double UnixMs(long fileTime) { return (fileTime - 116444736000000000L) / 10000.0; }

    public static object Run(string binary, string cwd, string[] argv, string stdoutPath,
        string stderrPath, int stdoutLimit, int stderrLimit, double deadlineUnixMs) {
        if (stdoutLimit < 1 || stdoutLimit > 16777216 || stderrLimit < 1 || stderrLimit > 1048576)
            throw new ArgumentException("Invalid output limits");
        // A pipe closing on parent exit also cancels. This task never logs input.
        Task<int> cancellation = Task.Run(() => Console.In.Read());
        double remaining = deadlineUnixMs - UnixMs(DateTime.UtcNow.ToFileTimeUtc());
        if (remaining <= 0 || cancellation.IsCompleted)
            return new { timedOut = remaining <= 0, cancelled = cancellation.IsCompleted, launched = false };

        var info = new ProcessStartInfo {
            FileName = binary, WorkingDirectory = cwd, Arguments = string.Join(" ", argv.Select(Quote)),
            UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true
        };
        using (var child = new Process { StartInfo = info }) {
            var timer = Stopwatch.StartNew();
            if (!child.Start()) throw new InvalidOperationException("CLI failed to start");
            // Retain the actual child's handle through exit and accounting; never reopen by PID.
            IntPtr handle = child.Handle;
            child.StandardInput.Close();
            var stdout = new Capture(); var stderr = new Capture();
            bool timedOut = false, cancelled = false, drainTimedOut = false;
            ulong peakWorkingSet = 0, peakPrivateCommit = 0;
            int memoryObservations = 0, memoryError = 0;
            bool finalMemoryRead = false;
            try {
                stdout.task = Drain(child.StandardOutput.BaseStream, stdoutPath, stdoutLimit, stdout);
                stderr.task = Drain(child.StandardError.BaseStream, stderrPath, stderrLimit, stderr);
                while (true) {
                    bool exited = child.HasExited;
                    MemoryCounters memory;
                    bool read = GetProcessMemoryInfo(handle, out memory, (uint)Marshal.SizeOf(typeof(MemoryCounters)));
                    if (read) {
                        memoryObservations++;
                        peakWorkingSet = Math.Max(peakWorkingSet, memory.peakWorkingSet.ToUInt64());
                        peakPrivateCommit = Math.Max(peakPrivateCommit, memory.peakPagefile.ToUInt64());
                    } else { memoryError = Marshal.GetLastWin32Error(); }
                    if (exited) { finalMemoryRead = read; break; }
                    timedOut = timer.Elapsed.TotalMilliseconds >= remaining;
                    cancelled = cancellation.IsCompleted;
                    if (timedOut || cancelled || stdout.exceeded || stderr.exceeded || stdout.error != null || stderr.error != null) {
                        // Process.Kill uses the retained child, never a name or unvalidated PID.
                        try { child.Kill(); } catch (InvalidOperationException) { if (!child.HasExited) throw; }
                        if (!child.WaitForExit(5000)) throw new TimeoutException("Owned child did not exit after kill");
                    } else { child.WaitForExit(10); }
                }
                if (!Task.WaitAll(new[] { stdout.task, stderr.task }, 2000)) {
                    drainTimedOut = true;
                    child.StandardOutput.Close(); child.StandardError.Close();
                    Task.WaitAll(new[] { stdout.task, stderr.task }, 1000);
                }
                long created, exitedAt, kernel, user;
                bool timesOk = GetProcessTimes(handle, out created, out exitedAt, out kernel, out user);
                int timesError = timesOk ? 0 : Marshal.GetLastWin32Error();
                IoCounters io;
                bool ioOk = GetProcessIoCounters(handle, out io);
                int ioError = ioOk ? 0 : Marshal.GetLastWin32Error();
                return new {
                    launched = true, pid = child.Id, exitCode = child.ExitCode, timedOut, cancelled,
                    outputLimitExceeded = stdout.exceeded || stderr.exceeded, drainTimedOut,
                    stdoutBytes = Interlocked.Read(ref stdout.bytes), stderrBytes = Interlocked.Read(ref stderr.bytes),
                    outputError = stdout.error ?? stderr.error,
                    nativeStartedUnixMs = timesOk ? (double?)UnixMs(created) : null,
                    nativeEndedUnixMs = timesOk ? (double?)UnixMs(exitedAt) : null,
                    nativeElapsedWallMs = timesOk ? (double?)((exitedAt - created) / 10000.0) : null,
                    cpu = new { source = "GetProcessTimes", userMs = timesOk ? (double?)(user / 10000.0) : null,
                        kernelMs = timesOk ? (double?)(kernel / 10000.0) : null,
                        totalMs = timesOk ? (double?)((kernel + user) / 10000.0) : null,
                        unavailableReason = timesOk ? null : "GetProcessTimes Win32 error " + timesError },
                    memory = new { source = "GetProcessMemoryInfo", sampleIntervalMs = 10, observations = memoryObservations,
                        peakWorkingSetBytes = memoryObservations > 0 ? (ulong?)peakWorkingSet : null,
                        peakPrivateCommitBytes = memoryObservations > 0 ? (ulong?)peakPrivateCommit : null,
                        complete = finalMemoryRead,
                        coverage = finalMemoryRead ? "lifetime-peak-at-exit" : "last-observed-lifetime-peak",
                        unavailableReason = memoryObservations > 0 ? null : "GetProcessMemoryInfo Win32 error " + memoryError,
                        finalReadError = finalMemoryRead ? null : "GetProcessMemoryInfo Win32 error " + memoryError },
                    io = new { source = "GetProcessIoCounters", readOperations = ioOk ? (ulong?)io.readOperations : null,
                        writeOperations = ioOk ? (ulong?)io.writeOperations : null, otherOperations = ioOk ? (ulong?)io.otherOperations : null,
                        readBytes = ioOk ? (ulong?)io.readBytes : null, writeBytes = ioOk ? (ulong?)io.writeBytes : null,
                        otherBytes = ioOk ? (ulong?)io.otherBytes : null,
                        unavailableReason = ioOk ? null : "GetProcessIoCounters Win32 error " + ioError }
                };
            } finally {
                if (!child.HasExited) {
                    try { child.Kill(); } catch (InvalidOperationException) { if (!child.HasExited) throw; }
                    child.WaitForExit(5000);
                }
            }
        }
    }
}
'@
    # No native child exists until the parent acknowledges readiness. This lets
    # Node bound launcher/Add-Type startup without risking an orphaned CLI.
    [Console]::WriteLine('{"ready":true}')
    [Console]::Out.Flush()
    if ([Console]::ReadLine() -cne 'start') {
        [Console]::WriteLine('{"launched":false,"cancelled":true}')
        exit 0
    }
    $result = [WorktreeProcessProbe]::Run(
        [string]$request.binary, [string]$request.cwd, [string[]]$request.argv,
        [string]$request.stdoutPath, [string]$request.stderrPath,
        [int]$request.stdoutLimit, [int]$request.stderrLimit, [double]$request.deadlineUnixMs)
    $result | ConvertTo-Json -Depth 6 -Compress
} catch {
    @{ helperError = $_.Exception.Message } | ConvertTo-Json -Compress
    exit 1
}
