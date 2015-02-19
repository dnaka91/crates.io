#![feature(core)]
#![cfg_attr(test, feature(old_io))]

extern crate conduit;

use std::error::Error;

use conduit::{Request, Response, Handler};

pub trait Middleware: Send + Sync + 'static {
    fn before(&self, _: &mut Request) -> Result<(), Box<Error+Send>> {
        Ok(())
    }

    fn after(&self, _: &mut Request, res: Result<Response, Box<Error+Send>>)
             -> Result<Response, Box<Error+Send>>
    {
        res
    }
}

pub trait AroundMiddleware: Handler {
    fn with_handler(&mut self, handler: Box<Handler + Send + Sync>);
}

pub struct MiddlewareBuilder {
    middlewares: Vec<Box<Middleware + Send + Sync>>,
    handler: Option<Box<Handler + Send + Sync>>
}

impl MiddlewareBuilder {
    pub fn new<H: Handler>(handler: H) -> MiddlewareBuilder {
        MiddlewareBuilder {
            middlewares: vec!(),
            handler: Some(Box::new(handler) as Box<Handler + Send + Sync>)
        }
    }

    pub fn add<M: Middleware>(&mut self, middleware: M) {
        self.middlewares.push(Box::new(middleware) as Box<Middleware + Send + Sync>);
    }

    pub fn around<M: AroundMiddleware>(&mut self, mut middleware: M) {
        let handler = self.handler.take().unwrap();
        middleware.with_handler(handler);
        self.handler = Some(Box::new(middleware) as Box<Handler + Send + Sync>);
    }
}

impl Handler for MiddlewareBuilder {
    fn call(&self, req: &mut Request) -> Result<Response, Box<Error+Send>> {
        let mut error = None;

        for (i, middleware) in self.middlewares.iter().enumerate() {
            match middleware.before(req) {
                Ok(_) => (),
                Err(err) => {
                    error = Some((err, i));
                    break;
                }
            }
        }

        match error {
            Some((err, i)) => {
                let middlewares = &self.middlewares[..i];
                run_afters(middlewares, req, Err(err))
            },
            None => {
                let res = { self.handler.as_ref().unwrap().call(req) };
                let middlewares = self.middlewares.as_slice();

                run_afters(middlewares, req, res)
            }
        }
    }
}

fn run_afters(middleware: &[Box<Middleware>],
                  req: &mut Request,
                  res: Result<Response, Box<Error+Send>>)
                  -> Result<Response, Box<Error+Send>>
{
    middleware.iter().rev().fold(res, |res, m| m.after(req, res))
}

#[cfg(test)]
mod tests {
    extern crate semver;

    use {MiddlewareBuilder, Middleware, AroundMiddleware};

    use std::collections::HashMap;
    use std::error::Error;
    use std::old_io::net::ip::IpAddr;
    use std::old_io::{self, MemReader};

    use conduit;
    use conduit::{Request, Response, Host, Headers, Method, Scheme, Extensions};
    use conduit::{Handler, TypeMap};

    struct RequestSentinel {
        path: String,
        extensions: TypeMap,
        method: Method
    }

    impl RequestSentinel {
        fn new(method: Method, path: &'static str) -> RequestSentinel {
            RequestSentinel {
                path: path.to_string(),
                extensions: TypeMap::new(),
                method: method
            }
        }
    }

    impl conduit::Request for RequestSentinel {
        fn http_version(&self) -> semver::Version { unimplemented!() }
        fn conduit_version(&self) -> semver::Version { unimplemented!() }
        fn method(&self) -> Method { self.method }
        fn scheme(&self) -> Scheme { unimplemented!() }
        fn host<'a>(&'a self) -> Host<'a> { unimplemented!() }
        fn virtual_root<'a>(&'a self) -> Option<&'a str> { unimplemented!() }
        fn path<'a>(&'a self) -> &'a str {
            self.path.as_slice()
        }
        fn query_string<'a>(&'a self) -> Option<&'a str> { unimplemented!() }
        fn remote_ip(&self) -> IpAddr { unimplemented!() }
        fn content_length(&self) -> Option<u64> { unimplemented!() }
        fn headers<'a>(&'a self) -> &'a Headers { unimplemented!() }
        fn body<'a>(&'a mut self) -> &'a mut Reader { unimplemented!() }
        fn extensions<'a>(&'a self) -> &'a Extensions {
            &self.extensions
        }
        fn mut_extensions<'a>(&'a mut self) -> &'a mut Extensions {
            &mut self.extensions
        }
    }

    struct MyMiddleware;

    impl Middleware for MyMiddleware {
        fn before<'a>(&self, req: &'a mut Request) -> Result<(), Box<Error+Send>> {
            req.mut_extensions().insert("hello".to_string());
            Ok(())
        }
    }

    struct ErrorRecovery;

    impl Middleware for ErrorRecovery {
        fn after(&self, _: &mut Request, res: Result<Response, Box<Error+Send>>)
                     -> Result<Response, Box<Error+Send>>
        {
            res.or_else(|e| {
                let e = e.description().to_string();
                Ok(Response {
                    status: (500, "Internal Server Error"),
                    headers: HashMap::new(),
                    body: Box::new(MemReader::new(e.into_bytes()))
                })
            })
        }
    }

    struct ProducesError;

    impl Middleware for ProducesError {
        fn before(&self, _: &mut Request) -> Result<(), Box<Error+Send>> {
            Err(Box::new(old_io::standard_error(old_io::OtherIoError)) as Box<Error+Send>)
        }
    }

    struct NotReached;

    impl Middleware for NotReached {
        fn after(&self, _: &mut Request, _: Result<Response, Box<Error+Send>>)
                     -> Result<Response, Box<Error+Send>>
        {
            Ok(Response {
                status: (200, "OK"),
                headers: HashMap::new(),
                body: Box::new(MemReader::new(vec!()))
            })
        }
    }

    struct MyAroundMiddleware {
        handler: Option<Box<Handler + Send + Sync>>
    }

    impl MyAroundMiddleware {
        fn new() -> MyAroundMiddleware {
            MyAroundMiddleware { handler: None }
        }
    }

    impl Middleware for MyAroundMiddleware {}

    impl AroundMiddleware for MyAroundMiddleware {
        fn with_handler(&mut self, handler: Box<Handler + Send + Sync>) {
            self.handler = Some(handler)
        }
    }

    impl Handler for MyAroundMiddleware {
        fn call(&self, req: &mut Request) -> Result<Response, Box<Error+Send>> {
            req.mut_extensions().insert("hello".to_string());
            self.handler.as_ref().unwrap().call(req)
        }
    }

    fn get_extension<'a, T: 'static>(req: &'a Request) -> &'a T {
        req.extensions().find::<T>().unwrap()
    }

    fn response(string: String) -> Response {
        Response {
            status: (200, "OK"),
            headers: HashMap::new(),
            body: Box::new(MemReader::new(string.into_bytes()))
        }
    }

    fn handler(req: &mut Request) -> Result<Response, old_io::IoError> {
        let hello = get_extension::<String>(req);
        Ok(response(hello.clone()))
    }

    fn error_handler(_: &mut Request) -> Result<Response, old_io::IoError> {
        Err(old_io::IoError {
            kind: old_io::OtherIoError,
            desc: "Error in handler",
            detail: None,
        })
    }

    fn middle_handler(req: &mut Request) -> Result<Response, old_io::IoError> {
        let hello = get_extension::<String>(req);
        let middle = get_extension::<String>(req);

        Ok(response(format!("{} {}", hello, middle)))
    }

    #[test]
    fn test_simple_middleware() {
        let mut builder = MiddlewareBuilder::new(handler);
        builder.add(MyMiddleware);

        let mut req = RequestSentinel::new(Method::Get, "/");
        let mut res = builder.call(&mut req).ok().expect("No response");

        assert_eq!(res.body.read_to_string().ok().expect("No body"), "hello".to_string());
    }

    #[test]
    fn test_error_recovery() {
        let mut builder = MiddlewareBuilder::new(handler);
        builder.add(ErrorRecovery);
        builder.add(ProducesError);
        // the error bubbles up from ProducesError and shouldn't reach here
        builder.add(NotReached);

        let mut req = RequestSentinel::new(Method::Get, "/");
        let res = builder.call(&mut req).ok().expect("Error not handled");

        assert_eq!(res.status, (500, "Internal Server Error"));
    }

    #[test]
    fn test_error_recovery_in_handlers() {
        let mut builder = MiddlewareBuilder::new(error_handler);
        builder.add(ErrorRecovery);

        let mut req = RequestSentinel::new(Method::Get, "/");
        let mut res = builder.call(&mut req).ok().expect("Error not handled");

        assert_eq!(res.status, (500, "Internal Server Error"));
        assert_eq!(res.body.read_to_string().ok().expect("No body"), "Error in handler".to_string());
    }

    #[test]
    fn test_around_middleware() {
        let mut builder = MiddlewareBuilder::new(middle_handler);
        builder.add(MyMiddleware);
        builder.around(MyAroundMiddleware::new());

        let mut req = RequestSentinel::new(Method::Get, "/");
        let mut res = builder.call(&mut req).ok().expect("No response");

        assert_eq!(res.body.read_to_string().ok().expect("No body"), "hello hello".to_string());
    }
}
