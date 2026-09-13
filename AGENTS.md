# sid — instrucciones de trabajo

- Después de modificar sid, compila siempre la versión que el usuario ejecuta
  para que pueda probar los cambios antes de dar el trabajo por terminado.
- Actualmente `~/.local/bin/sid` apunta a `target/release/sid`: ejecuta
  `cargo build --release --locked -p helix-term --bin sid` y comprueba que termine
  correctamente. Compilar solo `target/debug/sid` no basta.
- Si cambia la instalación, comprueba a qué binario apunta `sid` y actualiza ese
  destino. Indica al usuario que debe reiniciar sid para cargar el nuevo binario.
- Un cambio exclusivamente documental no necesita recompilar.
