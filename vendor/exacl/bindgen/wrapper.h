#include <sys/types.h>
#include <sys/errno.h>
#include <sys/acl.h>
#include <fcntl.h>
#if __APPLE__
// MacOS makes us translate between GUID and UID/GID.
# include <membership.h>
#elif __linux__
// Linux supplies non-standard ACL extensions in a different header.
# include <acl/libacl.h>
#endif
#include <grp.h>
#include <pwd.h>
#include <unistd.h>
