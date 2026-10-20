# Novedades de cada versión

`<versión>.md` (p. ej. `0.10.0.md`) es el texto de la versión en GitHub: lo
añade el workflow Release al publicarla, con la lista de cambios de GitHub
detrás. La aplicación de escritorio enseña, al avisar de la versión nueva, la
sección del idioma de la interfaz (o la inglesa si no está).

Cada sección empieza con su marca `<!-- lang:xx -->` (es, en, fr, it, pt, de)
y llega hasta la siguiente; el título de la sección va después de la marca:

```markdown
<!-- lang:es -->
## Español
- Bloquear el documento al firmar con certificado.

<!-- lang:en -->
## English
- Lock the document when signing with a certificate.
```

Sirve también para el texto "Novedades" de Google Play.
