# Developer documentation

## Making a release

1. Bump version in Cargo.toml
2. jj ci -m "release: version 0.x.0"
3. jj b m main --to @-
4. jj git push -r @-
5. git tag v0.x.0 <commit>
6. git push origin v0.x.0
