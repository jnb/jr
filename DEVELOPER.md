# Developer documentation

## Making a release

1. Bump version in Cargo.toml
2. jj ci -m "release: version 0.x.0"
3. git tag v0.x.0 <commit>
4. git push origin v0.x.0
