$llvm = 'C:\Users\user\AppData\Local\Microsoft\WinGet\Packages\MartinStorsjo.LLVM-MinGW.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\llvm-mingw-20260908-ucrt-x86_64\bin'
$env:Path = "$llvm;$env:Path"
cargo check -p pdfcraft-cli
cargo check -p pdfcraft-evo
