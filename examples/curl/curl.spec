# The same package as a conventional spec, for comparison.
# This is deliberately a "reasonable modern spec", not a production Fedora
# spec: it shows the macro/conditional/%files style that the recipe replaces.
# It is not tested.

%global _hardened_build 1

%bcond ssl 1
%bcond gnutls 0
%bcond http2 1
%bcond idn 0

Name:           curl
Version:        8.11.1
Release:        1%{?dist}
Summary:        A command line tool for transferring data with URLs
License:        curl
URL:            https://curl.se/
Source0:        https://curl.se/download/%{name}-%{version}.tar.xz
Source1:        https://curl.se/download/%{name}-%{version}.tar.xz.asc
Patch0:         0001-curl-ca-bundle-path.patch
BuildRequires:  cmake >= 3.20
BuildRequires:  ninja-build
BuildRequires:  pkgconf
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  zlib-devel
BuildRequires:  libpsl-devel

%if %{with ssl}
BuildRequires:  openssl-devel >= 3.0
%endif
%if %{with gnutls}
BuildRequires:  gnutls-devel >= 3.8
%endif
%if %{with http2}
BuildRequires:  libnghttp2-devel
%endif
%if %{with idn}
BuildRequires:  libidn2-devel
%endif

%description
curl is a command line tool and library for transferring data with URLs.
It supports HTTP, HTTPS, FTP, SFTP, SCP, LDAP, MQTT and more.

%package -n libcurl
Summary:        Library for transferring data with URLs
Requires:       ca-certificates
Provides:       libcurl.so.4()(64bit)

%description -n libcurl
libcurl is a client-side URL transfer library.

%package -n libcurl-devel
Summary:        Development files for libcurl
Requires:       libcurl%{?_isa} = %{epoch}:%{version}-%{release}

%description -n libcurl-devel
Headers and pkg-config files for libcurl.

%package -n curl-doc
Summary:        Documentation for curl
BuildArch:      noarch

%description -n curl-doc
Documentation and examples for curl.

%prep
%autosetup -p1

%build
%cmake \
  -DCMAKE_BUILD_TYPE=Release \
  -DCURL_HIDDEN_SYMBOLS=ON \
  -DCURL_USE_OPENSSL=%{?with_ssl:ON}%{!?with_ssl:OFF} \
  -DCURL_USE_GNUTLS=%{?with_gnutls:ON}%{!?with_gnutls:OFF} \
  -DCURL_USE_NGHTTP2=%{?with_http2:ON}%{!?with_http2:OFF} \
  -DCURL_USE_LIBIDN2=%{?with_idn:ON}%{!?with_idn:OFF} \
  -DBUILD_TESTING=ON
%cmake_build

%install
%cmake_install
rm -rf %{buildroot}%{_docdir}/curl/CHANGES

%check
ctest --output-on-failure --test-dir build

%post -n libcurl -p /sbin/ldconfig
%postun -n libcurl -p /sbin/ldconfig

%files
%{_bindir}/curl

%files -n libcurl
%{_libdir}/libcurl.so.4*

%files -n libcurl-devel
%{_includedir}/curl/
%{_libdir}/libcurl.so
%{_libdir}/pkgconfig/libcurl.pc

%files -n curl-doc
%{_docdir}/curl/
%{_mandir}/man1/curl.1*

%changelog
* Fri Oct 31 2026 Jane Packager <jane@example.org> - 8.11.1-1
- Initial package.
