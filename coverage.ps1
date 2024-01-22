Remove-Item *.profraw
Remove-Item *.profdata

cargo test
cargo profdata -- merge *.profraw -o cerboy.profdata
cargo cov -- show -Xdemangler=rustfilt -output-dir .\coverage -instr-profile='cerboy.profdata' -show-line-counts-or-regions -show-instantiations -format=html .\target\debug\deps\*.exe -sources .\src

Start-Process .\coverage\index.html
