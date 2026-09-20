# Sync V3 schema examples

These fixtures are structural JSON Schema examples. Repeated hexadecimal IDs are
readable placeholders; canonical digest conformance is additionally checked by
encoder golden tests during implementation. The examples deliberately exercise
bootstrap/View and PolicySet roots, an authorization snapshot, a no-op
RepositoryView CAS receipt, a closed pre-semantic transaction error, a typed PolicyRoot event, upload/fetch capability
binding, FileId-to-FileManifest selection and the exact SCM Layer record inside a
product envelope.
