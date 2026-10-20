#!/usr/bin/env bash
# Compila la aplicación de Android en local: el núcleo en Rust
# (build-rust.sh) y el APK de release con el Gradle wrapper.
#
#   scripts/android/compilar.sh            # APK en dist/
#   scripts/android/compilar.sh bundle     # también el App Bundle (.aab)
#
# Usa el SDK de ~/Android/Sdk (o ANDROID_HOME), el NDK más reciente que haya
# en él (o ANDROID_NDK_HOME) y Java 17 (o JAVA_HOME): Gradle 8.11 no funciona
# con versiones de Java más nuevas. Sin la clave de subida (ANDROID_KEYSTORE,
# ver crear-clave-de-subida.sh) se firma con la clave de depuración.
set -euo pipefail
cd "$(dirname "$0")/../.."

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  ANDROID_NDK_HOME=$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)
  export ANDROID_NDK_HOME
fi
export JAVA_HOME="${JAVA_HOME:-/usr/lib/jvm/java-17-openjdk}"
for d in "$ANDROID_HOME" "$ANDROID_NDK_HOME" "$JAVA_HOME"; do
  [[ -d "$d" ]] || { echo "No existe $d" >&2; exit 1; }
done

scripts/android/build-rust.sh
tasks=(assembleRelease)
[[ "${1:-}" == bundle ]] && tasks+=(bundleRelease)
android/gradlew -p android "${tasks[@]}"

V=$(grep -m1 'versionName' android/app/build.gradle.kts | cut -d'"' -f2)
mkdir -p dist
cp android/app/build/outputs/apk/release/app-release.apk "dist/form-pdf-reader-$V-android-arm64.apk"
echo "Creado dist/form-pdf-reader-$V-android-arm64.apk"
if [[ "${1:-}" == bundle ]]; then
  cp android/app/build/outputs/bundle/release/app-release.aab "dist/form-pdf-reader-$V-android.aab"
  echo "Creado dist/form-pdf-reader-$V-android.aab"
fi
