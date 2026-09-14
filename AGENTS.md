# sid — instrucciones de trabajo

- **Siempre que se modifica el código de sid hay que recompilar**: ningún cambio
  llega al editor que usa el usuario hasta que se compila y se reinicia sid.
  Hazlo antes de dar el trabajo por terminado para que pueda probarlo.
- Recompila con `./build.sh` (equivale a
  `cargo build --release --locked -p helix-term --bin sid` y enlaza
  `~/.local/bin/sid` a `target/release/sid`) y comprueba que termine
  correctamente. Compilar solo `target/debug/sid` no basta.
- Si cambia la instalación, comprueba a qué binario apunta `sid` y actualiza ese
  destino. Indica al usuario que debe reiniciar sid para cargar el nuevo binario.
- Un cambio exclusivamente documental no necesita recompilar.
