# Coil — instrucciones de trabajo

- Después de modificar Coil, compila siempre la versión que el usuario ejecuta
  para que pueda probar los cambios antes de dar el trabajo por terminado.
- Actualmente `~/.local/bin/coil` apunta a `target/release/coil`: ejecuta
  `cargo build --release --locked -p helix-term --bin coil` y comprueba que termine
  correctamente. Compilar solo `target/debug/coil` no basta.
- Si cambia la instalación, comprueba a qué binario apunta `coil` y actualiza ese
  destino. Indica al usuario que debe reiniciar Coil para cargar el nuevo binario.
- Un cambio exclusivamente documental no necesita recompilar.
