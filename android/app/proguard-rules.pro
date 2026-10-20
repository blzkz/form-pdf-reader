# Reglas de R8 (ofuscación y reducción del código de la versión release).

# JNA, con la que los bindings de uniffi llaman al núcleo en Rust: su parte
# nativa (libjnidispatch) usa sus clases por nombre, y JNA lee por reflexión
# los campos de las Structure (con @Structure.FieldOrder), los métodos de las
# interfaces Library (cada uno es una función de la librería nativa) y los
# Callback.
-keep class com.sun.jna.** { *; }
-dontwarn java.awt.**
-keepattributes RuntimeVisibleAnnotations,Signature,InnerClasses,EnclosingMethod
-keep interface * extends com.sun.jna.Library { *; }
-keep class * extends com.sun.jna.Structure { *; }
-keep interface * extends com.sun.jna.Callback { *; }
-keep class * implements com.sun.jna.Callback { *; }

# Trazas de errores legibles con el mapping que se sube con el App Bundle.
-keepattributes SourceFile,LineNumberTable
-renamesourcefileattribute SourceFile
