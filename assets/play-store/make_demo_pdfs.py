#!/usr/bin/env python3
"""Demo PDFs for the store screenshots: a filled-in AcroForm and a 4-page guide.

Plain PDF written by hand (Helvetica, WinAnsi), so the text is selectable and
searchable and the form fields are real AcroForm fields.
"""
import sys
import zlib

W, H = 595, 842  # A4 in points


def esc(s):
    return s.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")


class Pdf:
    def __init__(self):
        self.objs = []

    def add(self, body):
        self.objs.append(body)
        return len(self.objs)

    def set(self, n, body):
        self.objs[n - 1] = body

    def stream(self, data, extra=""):
        raw = data.encode("latin-1")
        comp = zlib.compress(raw)
        return f"<< /Length {len(comp)} /Filter /FlateDecode {extra}>>\nstream\n".encode() + comp + b"\nendstream"

    def write(self, path, root):
        out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
        offs = []
        for i, body in enumerate(self.objs, 1):
            offs.append(len(out))
            out += f"{i} 0 obj\n".encode()
            out += body if isinstance(body, bytes) else body.encode("latin-1")
            out += b"\nendobj\n"
        x = len(out)
        out += f"xref\n0 {len(self.objs) + 1}\n0000000000 65535 f \n".encode()
        for o in offs:
            out += f"{o:010d} 00000 n \n".encode()
        out += f"trailer\n<< /Size {len(self.objs) + 1} /Root {root} 0 R >>\nstartxref\n{x}\n%%EOF\n".encode()
        open(path, "wb").write(out)


def text(x, y, s, size=11, font="F1", rgb=(0.12, 0.11, 0.09)):
    r, g, b = rgb
    return f"BT /{font} {size} Tf {r} {g} {b} rg {x} {y} Td ({esc(s)}) Tj ET\n"


def rect(x, y, w, h, fill=None, stroke=None, lw=1):
    s = f"{lw} w "
    if fill:
        s += f"{fill[0]} {fill[1]} {fill[2]} rg "
    if stroke:
        s += f"{stroke[0]} {stroke[1]} {stroke[2]} RG "
    op = "B" if fill and stroke else ("f" if fill else "S")
    return s + f"{x} {y} {w} {h} re {op}\n"


YELLOW = (0.957, 0.706, 0.0)
DARK = (0.149, 0.102, 0.0)
GREY = (0.45, 0.43, 0.40)
LINE = (0.80, 0.78, 0.74)


def fonts(pdf):
    f1 = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
    f2 = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>")
    zd = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>")
    return f1, f2, zd


def form(path):
    pdf = Pdf()
    f1, f2, zd = fonts(pdf)
    catalog = pdf.add("")
    pages = pdf.add("")
    page = pdf.add("")
    fields = []
    c = ""
    # Header band.
    c += rect(0, H - 110, W, 110, fill=YELLOW)
    c += text(48, H - 58, "Riverside Community Library", 22, "F2", DARK)
    c += text(48, H - 84, "Membership application form", 13, "F1", DARK)
    c += text(48, H - 150, "Please fill in all the fields marked with *. Your card will be ready within", 10, "F1", GREY)
    c += text(48, H - 164, "five working days and you will receive an e-mail when you can collect it.", 10, "F1", GREY)

    def section(y, title):
        return text(48, y, title, 13, "F2", DARK) + rect(48, y - 8, W - 96, 0.8, fill=LINE)

    def tx(name, label, x, y, w, value, h=24):
        nonlocal c
        c += text(x, y + h + 6, label, 9, "F1", GREY)
        n = pdf.add(
            f"<< /Type /Annot /Subtype /Widget /FT /Tx /T ({esc(name)}) /V ({esc(value)}) /F 4 "
            f"/Rect [{x} {y} {x + w} {y + h}] /DA (/Helv 11 Tf 0.1 0.1 0.1 rg) "
            f"/MK << /BC [0.75 0.73 0.70] /BG [1 0.98 0.93] >> /P {page} 0 R >>"
        )
        fields.append(n)

    def cb(name, label, x, y, on):
        nonlocal c
        size = 13
        ap_on = pdf.add(pdf.stream(
            f"1 0.98 0.93 rg 0 0 {size} {size} re f 0.75 0.73 0.70 RG 1 w 0.5 0.5 {size-1} {size-1} re S "
            f"BT /ZaDb 11 Tf 0.15 0.10 0 rg 1.6 2.3 Td (4) Tj ET",
            f"/Type /XObject /Subtype /Form /BBox [0 0 {size} {size}] /Resources << /Font << /ZaDb {zd} 0 R >> >> "))
        ap_off = pdf.add(pdf.stream(
            f"1 0.98 0.93 rg 0 0 {size} {size} re f 0.75 0.73 0.70 RG 1 w 0.5 0.5 {size-1} {size-1} re S",
            f"/Type /XObject /Subtype /Form /BBox [0 0 {size} {size}] "))
        state = "/Yes" if on else "/Off"
        n = pdf.add(
            f"<< /Type /Annot /Subtype /Widget /FT /Btn /T ({esc(name)}) /V {state} /AS {state} /F 4 "
            f"/Rect [{x} {y} {x + size} {y + size}] /MK << /CA (4) /BC [0.75 0.73 0.70] /BG [1 0.98 0.93] >> "
            f"/DA (/ZaDb 0 Tf 0 g) /AP << /N << /Yes {ap_on} 0 R /Off {ap_off} 0 R >> >> /P {page} 0 R >>"
        )
        fields.append(n)
        c += text(x + size + 8, y + 2, label, 11, "F1", DARK)

    y = H - 210
    c += section(y, "Personal details")
    tx("first_name", "First name *", 48, y - 52, 240, "Alex")
    tx("last_name", "Last name *", 307, y - 52, 240, "Morgan")
    tx("birth_date", "Date of birth *", 48, y - 104, 150, "14/03/1992")
    tx("id_number", "ID or passport number *", 217, y - 104, 330, "X1234567L")
    y -= 150
    c += section(y, "Contact")
    tx("email", "E-mail *", 48, y - 52, 300, "alex.morgan@example.com")
    tx("phone", "Phone", 367, y - 52, 180, "+44 20 7946 0958")
    tx("address", "Address", 48, y - 104, 499, "12 Willow Lane, Riverside")
    y -= 150
    c += section(y, "Membership")
    cb("type_adult", "Adult", 48, y - 34, True)
    cb("type_student", "Student", 160, y - 34, False)
    cb("type_senior", "Senior (65+)", 280, y - 34, False)
    cb("events", "Send me news about events and new books", 48, y - 62, True)
    tx("interests", "Favourite topics", 48, y - 118, 499, "History, science fiction, cooking")
    y -= 170
    c += section(y, "Declaration")
    c += text(48, y - 30, "I confirm that the information above is correct and I accept the library rules.", 10, "F1", GREY)
    tx("date", "Date *", 48, y - 82, 150, "12/10/2026")
    tx("signature", "Name of the applicant *", 217, y - 82, 330, "Alex Morgan")
    c += text(48, 40, "Riverside Community Library - fictitious form for demonstration purposes.", 8, "F1", GREY)

    content = pdf.add(pdf.stream(c))
    annots = " ".join(f"{n} 0 R" for n in fields)
    pdf.set(page, f"<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 {W} {H}] /Contents {content} 0 R "
                  f"/Resources << /Font << /F1 {f1} 0 R /F2 {f2} 0 R /ZaDb {zd} 0 R >> >> /Annots [{annots}] >>")
    pdf.set(pages, f"<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>")
    pdf.set(catalog, f"<< /Type /Catalog /Pages {pages} 0 R /AcroForm << /Fields [{annots}] /NeedAppearances true "
                     f"/DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv {f1} 0 R /ZaDb {zd} 0 R >> >> >> >>")
    pdf.write(path, catalog)


GUIDE = [
    ("Filling in PDF forms", [
        "Many forms that public offices, schools and associations publish are PDF documents with fields you can fill in.",
        "Some are simple AcroForm forms. Others are dynamic XFA forms, which add sections and rows as you answer.",
        "Before you start, save a copy of the blank form so you can always start again.",
        "Read the instructions on the first page: they usually explain which fields are required and how to submit the form.",
    ]),
    ("Before you submit", [
        "Check every required field. Most forms mark them with an asterisk and highlight the ones that are missing.",
        "If the form has a Validate button, press it: it checks the dates, the numbers and the attachments for you.",
        "Attach the documents the form asks for, such as certificates or a copy of your ID, in the attachments section.",
        "Save the form again after the last change, and keep the saved file until your application is processed.",
    ]),
    ("Attachments and signatures", [
        "Files you attach to a form are stored inside the PDF, so you only need to send one document.",
        "Some forms need a digital signature. Sign the saved PDF with the tool your administration recommends.",
        "If the form cannot be sent from the document itself, upload the saved PDF to the website or office it indicates.",
        "Keep the receipt or registration number you get when you submit the form.",
    ]),
    ("Tips for reading on a tablet", [
        "Use the two-page view in landscape to read reports and guides like a book.",
        "Search finds every occurrence of a word; use the arrows to jump from one result to the next.",
        "A long press on a word selects it; drag the handles to select a whole paragraph and copy it.",
        "With a keyboard, Ctrl+F searches, Ctrl+S saves and Ctrl+Tab switches between open documents.",
    ]),
]


CHECKLIST = [
    ["Blank copy of the form saved", "Instructions read", "Required documents at hand", "Deadline noted in your calendar"],
    ["All required fields filled in", "Form validated without errors", "Attachments added", "Final version saved"],
    ["Attachments open correctly", "Document signed if required", "Form submitted", "Receipt kept"],
    ["Two-page view in landscape", "Search with Ctrl+F", "Text copied with a long press", "Documents open in tabs"],
]


def guide(path):
    pdf = Pdf()
    f1, f2, _ = fonts(pdf)
    catalog = pdf.add("")
    pages = pdf.add("")
    kids = []
    for i, (title, paras) in enumerate(GUIDE, 1):
        page = pdf.add("")
        c = rect(0, H - 70, W, 70, fill=YELLOW)
        c += text(48, H - 44, "Form filling guide", 14, "F2", DARK)
        c += text(W - 110, H - 44, f"Page {i} of {len(GUIDE)}", 10, "F1", DARK)
        c += text(48, H - 130, f"{i}. {title}", 22, "F2", DARK)
        y = H - 175
        for p in paras:
            words, line, lines = p.split(), "", []
            for w_ in words:
                if len(line) + len(w_) + 1 > 72:
                    lines.append(line)
                    line = w_
                else:
                    line = (line + " " + w_).strip()
            lines.append(line)
            for ln in lines:
                c += text(48, y, ln, 13, "F1", (0.15, 0.14, 0.12))
                y -= 20
            y -= 14
        c += rect(48, y - 90, W - 96, 80, fill=(1, 0.97, 0.88), stroke=YELLOW)
        c += text(64, y - 40, "Tip", 12, "F2", DARK)
        c += text(64, y - 60, "Save often: the form keeps everything you filled in.", 11, "F1", DARK)
        y -= 130
        c += text(48, y, "Checklist", 14, "F2", DARK)
        for item in CHECKLIST[i - 1]:
            y -= 26
            c += rect(48, y - 2, 12, 12, stroke=GREY)
            c += text(70, y, item, 12, "F1", (0.15, 0.14, 0.12))
        content = pdf.add(pdf.stream(c))
        pdf.set(page, f"<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 {W} {H}] /Contents {content} 0 R "
                      f"/Resources << /Font << /F1 {f1} 0 R /F2 {f2} 0 R >> >> >>")
        kids.append(page)
    pdf.set(pages, f"<< /Type /Pages /Kids [{' '.join(f'{k} 0 R' for k in kids)}] /Count {len(kids)} >>")
    pdf.set(catalog, f"<< /Type /Catalog /Pages {pages} 0 R >>")
    pdf.write(path, catalog)


if __name__ == "__main__":
    out = sys.argv[1]
    form(f"{out}/library-membership-form.pdf")
    guide(f"{out}/form-filling-guide.pdf")
