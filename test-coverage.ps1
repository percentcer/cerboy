$env:RUSTFLAGS="-C instrument-coverage"

# create the coverage data *.profraw
cargo test

# merge it into a format we can pass to cov
cargo profdata -- merge *.profraw -o cerboy.profdata

# generate coverage report
cargo cov -- show -Xdemangler=rustfilt -output-dir .\coverage -instr-profile='cerboy.profdata' $(Get-ChildItem .\target\debug\deps\*.exe | ForEach-Object {"-object=" + $_.FullName}) -show-line-counts-or-regions -show-instantiations -format=html -sources .\src

# clean up
Remove-Item *.profraw
Remove-Item *.profdata

Start-Process .\coverage\index.html
