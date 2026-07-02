# BiohazardFS Command Surface Draft

The CLI is agent-native and should return JSON by default.

Initial command sketch:

```bash
biohazardfs --version
biohazardfs commands schema --format json
biohazardfs config path
biohazardfs config show --redacted
biohazardfs config doctor
biohazardfs auth login
biohazardfs auth invite <code>
biohazardfs auth status
biohazardfs daemon status
biohazardfs daemon start
biohazardfs mount status
biohazardfs mount attach --path <mount-path>
biohazardfs mount detach
biohazardfs worksets list
biohazardfs cache status
biohazardfs cache pin <path>
biohazardfs cache dehydrate <path> --dry-run
biohazardfs cache dehydrate <path> --yes
biohazardfs transfers list
biohazardfs conflicts list
biohazardfs smoke run --format json
```
