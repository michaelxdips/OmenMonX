param([Parameter(Mandatory = $true)][string]$Path)
# Extract visible text from a screenshot with the Windows OCR engine.
#
# PowerShell 5.1 cannot await WinRT IAsyncOperation directly, so the bridge below
# converts each operation to a .NET Task and waits on it.
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
})[0]

function Await($op, [Type]$resultType) {
    $m = $asTaskGeneric.MakeGenericMethod($resultType)
    $task = $m.Invoke($null, @($op))
    $task.Wait(-1) | Out-Null
    $task.Result
}

[Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType = WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation, ContentType = WindowsRuntime] | Out-Null
[Windows.Storage.StorageFile, Windows.Foundation, ContentType = WindowsRuntime] | Out-Null
[Windows.Storage.FileAccessMode, Windows.Foundation, ContentType = WindowsRuntime] | Out-Null

$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($Path)) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])

$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
if ($null -eq $engine) { throw "no OCR engine for user languages" }
$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
$result.Text
