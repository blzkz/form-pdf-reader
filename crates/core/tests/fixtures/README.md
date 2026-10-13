# Test fixtures

- `normal.pdf`, `acroform.pdf`: made for these tests, MIT like the rest of
  the project.
- `TINTERNET_original.pdf`: blank application form published by the Banco de
  España, a real dynamic XFA form. It is not covered by the project's MIT
  licence and is included only as test material.
- `TINTERNET_adobe.pdf`: the same form filled in with made-up data and saved
  with Adobe Reader, with two attached files. Same terms as the blank form.

## corpus/

Blank public forms from other issuers, used by the `corpus` test (open,
render, fill a field, save, reopen). They are not covered by the project's
MIT licence; each keeps its own terms.

| File | Form | Issuer | Type | Terms | Source | SHA-256 |
|---|---|---|---|---|---|---|
| `hmrc-chv1.pdf` | ChV1, charities change of details | HM Revenue & Customs (UK) | Dynamic XFA | Open Government Licence v3.0 | [gov.uk](https://www.gov.uk/government/publications/charities-change-of-details-form-chv1) | `0747626f…27a7` |
| `us-army-da4856.pdf` | DA Form 4856, developmental counseling | US Army Publishing Directorate | Dynamic XFA, encrypted (owner password) | US Government work, public domain | [armypubs.army.mil](https://armypubs.army.mil/ProductMaps/PubForm/Details.aspx?PUB_ID=1026753) | `fce33b0c…f3e5` |
| `ec-mir-7.3.1.pdf` | MIR 7.3.1, manufacturer incident report | European Commission | Dynamic XFA, encrypted (owner password, AES-256) | © European Union, CC BY 4.0 | [health.ec.europa.eu](https://health.ec.europa.eu/medical-devices-sector/new-regulations/guidance-mdcg-endorsed-documents-and-other-guidance/pmsv-reporting-forms_en) | `fbf4c86a…397b` |
| `gencat-22100.pdf` | 22100, parking card for people with disabilities | Generalitat de Catalunya | Dynamic XFA | Open reuse with attribution ([legal notice](https://tramits.gencat.cat/ca/ajuda/avis_legal/)) | [dretssocials.gencat.cat](https://dretssocials.gencat.cat/ca/ambits_tematics/accessibilitat/mobilitat/targetes-aparcament/index.html) | `f97829f8…fda4` |
| `irs-f1040x.pdf` | Form 1040-X | Internal Revenue Service (US) | Static XFA | US Government work, public domain | [irs.gov](https://www.irs.gov/pub/irs-pdf/f1040x.pdf) | `e4d8d7df…013e` |
| `uscis-i9.pdf` | Form I-9 | US Citizenship and Immigration Services | AcroForm | US Government work, public domain | [uscis.gov](https://www.uscis.gov/i-9) | `780f348c…7281` |
