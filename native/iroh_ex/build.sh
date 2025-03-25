TARGET=aarch64-apple-ios-sim
#aarch64-apple-darwin

#RUSTFLAGS="-C debuginfo=0 -C lto=off -C embed-bitcode=no -C opt-level=z -C codegen-units=1 -C strip=symbols -C link-arg=-Wl,-dead_strip -C link-arg=-Wl,--gc-sections -C link-arg=-nostdlib -C panic=abort"
# --lib 
#cargo rustc --lib -Zbuild-std --target=aarch64-apple-ios --release --features rustler/staticlib --crate-type staticlib -- -C debuginfo=0 -C lto=off -C embed-bitcode=no -C opt-level=z -C codegen-units=1 -C strip=symbols -C link-arg=-Wl,-dead_strip -C link-arg=-Wl,--gc-sections -C link-arg=-nostdlib


# RUSTFLAGS="-C debuginfo=0 -C lto=off -C embed-bitcode=no -C opt-level=z -C codegen-units=1 -C strip=symbols -C link-arg=-Wl,-dead_strip -C link-arg=-Wl,--gc-sections -C link-arg=-nostdlib -C panic=abort"
# cargo rustc --lib --crate-type staticlib --target=aarch64-apple-darwin --features rustler/staticlib -- -C debuginfo=0 -C lto=off -C embed-bitcode=no -C opt-level=z-C codegen-units=1 -C strip=symbols -C link-arg=-Wl,-dead_strip -C link-arg=-Wl,--gc-sections -C link-arg=-nostdlib -C panic=abort

# RUSTFLAGS="-C debuginfo=0 -C lto=thin -C embed-bitcode=yes -C opt-level=z -C codegen-units=1 -C strip=symbols -C link-arg=-Wl,-dead_strip -C link-arg=-Wl,--gc-sections -C link-arg=-nostdlib -C panic=abort"
# cargo rustc --lib --crate-type staticlib --target=aarch64-apple-darwin --features rustler/staticlib -- -C debuginfo=0 -C lto=thin -C embed-bitcode=yes -C opt-level=z -C codegen-units=1 -C strip=symbols -C link-arg=-Wl,-dead_strip -C link-arg=-Wl,--gc-sections -C link-arg=-nostdlib -C panic=abort


#RUSTFLAGS="-C debuginfo=0 -C lto=off -C embed-bitcode=no -C opt-level=z -C codegen-units=1 -C strip=symbols -C link-arg=-Wl,-dead_strip -C link-arg=-Wl,--gc-sections -C link-arg=-nostdlib -C panic=abort"

#cargo rustc --lib --release --crate-type staticlib --target=$TARGET --features rustler/staticlib
cargo rustc --lib --release --crate-type dylib --target=$TARGET
