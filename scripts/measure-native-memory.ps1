param(
    [Parameter(Mandatory=$true)][string]$Binary,
    [Parameter(Mandatory=$true)][string]$FixtureRoot,
    [Parameter(Mandatory=$true)][string]$ReportPath
)
$ErrorActionPreference = 'Stop'
$binaryPath = (Resolve-Path -LiteralPath $Binary).Path
$fixturePath = (Resolve-Path -LiteralPath $FixtureRoot).Path
$reportFullPath = [IO.Path]::GetFullPath($ReportPath)
if (Test-Path -LiteralPath $reportFullPath) { throw 'Choose a new report path; existing measurements are preserved.' }
if ($binaryPath.Contains('"') -or $fixturePath.Contains('"')) { throw 'Paths containing quotes are unsupported.' }
$samples = @()
foreach ($count in @(10000,99000)) {
    $checkout = Join-Path $fixturePath "files-$count"
    if (-not (Test-Path -LiteralPath (Join-Path $checkout '.layrs') -PathType Container)) { throw 'Expected retained synthetic native fixture.' }
    foreach ($command in @('status','diff')) {
        $info = New-Object System.Diagnostics.ProcessStartInfo
        $info.FileName = $binaryPath
        $info.Arguments = "--json --space `"$checkout`" $command"
        $info.UseShellExecute = $false
        $info.CreateNoWindow = $true
        $info.RedirectStandardOutput = $true
        $info.RedirectStandardError = $true
        $process = New-Object System.Diagnostics.Process
        $process.StartInfo = $info
        $timer = [Diagnostics.Stopwatch]::StartNew()
        if (-not $process.Start()) { throw 'CLI did not start.' }
        $output = $process.StandardOutput.ReadToEndAsync()
        $errors = $process.StandardError.ReadToEndAsync()
        [long]$peakWorkingSet = 0
        [long]$sampledPrivateBytes = 0
        $observations = 0
        while (-not $process.WaitForExit(50)) {
            $process.Refresh()
            $peakWorkingSet = [Math]::Max($peakWorkingSet,$process.PeakWorkingSet64)
            $sampledPrivateBytes = [Math]::Max($sampledPrivateBytes,$process.PrivateMemorySize64)
            $observations++
            if ($timer.Elapsed.TotalMinutes -gt 15) { $process.Kill(); throw 'Measurement timed out.' }
        }
        $timer.Stop()
        $reply = $output.GetAwaiter().GetResult() | ConvertFrom-Json
        $null = $errors.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0 -or $reply.ok -ne $true) { throw "Native $command measurement failed." }
        $samples += [ordered]@{files=$count;command=$command;elapsedMs=$timer.ElapsedMilliseconds;peakWorkingSetBytes=$peakWorkingSet;sampledPeakPrivateBytes=$sampledPrivateBytes;observations=$observations}
        $process.Dispose()
    }
}
$report = [ordered]@{
    schemaVersion='layrs.local-memory-measurement.v1';measuredAt=[DateTime]::UtcNow.ToString('o')
    binarySha256=(Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant()
    fixtureRoot=$fixturePath;sampleIntervalMs=50;samples=$samples
    limitations=@('Windows process counters, sampled every 50ms; very short-lived peaks can escape private-byte sampling.','Read-only status/diff on retained repeated-content fixtures, warm caches.','Not a measurement of unique large files, remote sync, or all subprocesses.')
}
[IO.File]::WriteAllText($reportFullPath,($report | ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
Write-Output "Native memory report written: $reportFullPath"
